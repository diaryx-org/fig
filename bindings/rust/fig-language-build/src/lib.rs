//! Compiles a fig language written in Zig into the static library its
//! Rust crate links, `libfig_<name>.a`: the build-dependency of each
//! language crate (`fig-yaml`, …), and of one written outside fig.
//!
//! A language crate is a Zig package too (`build.zig` beside its
//! `Cargo.toml`), whose build depends on fig's kit and makes the library
//! with `zig build c` (see `language` in the kit's `build.zig`). Its build
//! script is one call:
//!
//! ```no_run
//! // build.rs
//! fn main() {
//!     fig_language_build::compile("yaml");
//! }
//! ```
//!
//! which builds the library for the target Cargo is building and links it,
//! beside fig's own (`fig-sys`). Building needs Zig 0.17 or later on `PATH`.
//!
//! The build happens in a copy of the package, in `OUT_DIR`, beside a copy
//! of the kit: fig's own (`kit/`, two directories up) for a language in
//! fig's tree, and otherwise the one this crate carries, which is the kit of
//! the fig release it belongs to. The copy's `build.zig.zon` names that kit
//! as `fig_kit`, keeps any dependency fetched by URL, and drops any other by
//! path, which a published crate has nothing at; a package whose tests need
//! one asks `b.available_deps` before it asks for it.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Compile the language `name` from the Zig package at the crate's root
/// (`CARGO_MANIFEST_DIR`), and link its library. Call it from the crate's
/// build script.
pub fn compile(name: &str) {
    // docs.rs has no Zig toolchain; `cargo doc` never links the library.
    if env::var_os("DOCS_RS").is_some() {
        return;
    }
    let cargo_target = env::var("TARGET").expect("Cargo sets TARGET");
    let cargo_host = env::var("HOST").expect("Cargo sets HOST");
    let package =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let prefix = out_dir.join("zig-prefix");
    let kit = kit_source(&package);
    let build_root = copy_package(&package, &kit, &out_dir.join("fig-language"));

    let mut command = Command::new("zig");
    command
        .arg("build")
        .arg("c")
        .arg("-Doptimize=ReleaseFast")
        .arg("-Dstrip=true")
        // The portable baseline CPU, as fig-sys's build pins it.
        .arg("-Dcpu=baseline");
    if let Some(zig_target) = zig_target_for_cargo_target(&cargo_target, &cargo_host) {
        command.arg(format!("-Dtarget={zig_target}"));
    }
    let status = command
        .arg("--prefix")
        .arg(&prefix)
        .current_dir(&build_root)
        .status()
        .unwrap_or_else(|e| panic!("compiling {name}'s library needs Zig 0.17+ (https://ziglang.org/download/), and running `zig` failed: {e}"));
    if !status.success() {
        panic!("`zig build c` for {name} failed with status {status}");
    }

    let lib = format!("fig_{name}");
    if cargo_target.contains("apple") {
        repack_archive_for_apple_ld(&prefix.join("lib").join(format!("lib{lib}.a")));
    }
    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static={lib}");
    for input in PACKAGE_INPUTS {
        println!("cargo:rerun-if-changed={}", package.join(input).display());
    }
    for input in KIT_INPUTS {
        println!("cargo:rerun-if-changed={}", kit.join(input).display());
    }
}

/// What of a language package its library is built from.
const PACKAGE_INPUTS: [&str; 4] = ["build.zig", "build.zig.zon", "src", "include"];
/// What of the kit a language is built against.
const KIT_INPUTS: [&str; 3] = ["build.zig", "build.zig.zon", "src"];

/// The kit to build `package` against: fig's own, when the package is one
/// of fig's languages (`languages/<name>/`, with `kit/` two directories
/// up), or else the copy this crate carries (`kit/`, put there by fig's
/// `zig build vendor-rust` before it is published).
fn kit_source(package: &Path) -> PathBuf {
    let in_tree = package.join("../../kit");
    if is_kit(&in_tree) {
        return in_tree;
    }
    let carried = Path::new(env!("CARGO_MANIFEST_DIR")).join("kit");
    if is_kit(&carried) {
        return carried;
    }
    panic!(
        "fig-language-build: no kit to build {} against: it is not in fig's tree, and this \
         crate carries none at {} (in fig's repository, `zig build vendor-rust` puts it there)",
        package.display(),
        carried.display()
    );
}

/// Whether `dir` is the root of fig's kit package.
fn is_kit(dir: &Path) -> bool {
    fs::read_to_string(dir.join("build.zig.zon")).is_ok_and(|zon| zon.contains(".name = .fig_kit"))
}

/// Copy `package` and `kit` side by side under `root`, with the package's
/// `build.zig.zon` rewritten to name the copied kit, and return where the
/// package's copy is.
fn copy_package(package: &Path, kit: &Path, root: &Path) -> PathBuf {
    let _ = fs::remove_dir_all(root);
    let (to_package, to_kit) = (root.join("package"), root.join("kit"));
    for (from, to, inputs) in [
        (package, &to_package, &PACKAGE_INPUTS[..]),
        (kit, &to_kit, &KIT_INPUTS[..]),
    ] {
        for input in inputs {
            let from = from.join(input);
            if from.exists() {
                copy_all(&from, &to.join(input));
            }
        }
    }
    let zon_path = to_package.join("build.zig.zon");
    let zon = fs::read_to_string(&zon_path).expect("a language package has a build.zig.zon");
    fs::write(&zon_path, with_kit_at(&zon, "../kit")).expect("write the copied build.zig.zon");
    to_package
}

/// Copy a file, or a directory and everything in it.
fn copy_all(from: &Path, to: &Path) {
    if from.is_dir() {
        fs::create_dir_all(to).expect("create a directory of the copy");
        for entry in fs::read_dir(from).expect("read a directory of the package") {
            let entry = entry.expect("a directory entry");
            copy_all(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).expect("create a directory of the copy");
        }
        fs::copy(from, to).expect("copy a file of the package");
    }
}

/// `zon` with its `.dependencies` naming the kit at `kit_path`: `fig_kit`'s
/// entry replaced, an entry fetched by URL kept as written, and any other
/// by path dropped.
fn with_kit_at(zon: &str, kit_path: &str) -> String {
    let kit = format!(".fig_kit = .{{ .path = \"{kit_path}\" }},");
    let Some(open) = zon
        .find(".dependencies")
        .and_then(|k| zon[k..].find('{').map(|o| k + o))
    else {
        // No dependencies at all: give it the kit's.
        let at = zon.rfind('}').expect("a build.zig.zon is one struct");
        return format!(
            "{}    .dependencies = .{{ {kit} }},\n{}",
            &zon[..at],
            &zon[at..]
        );
    };
    let close = matching_brace(zon, open);
    let mut body = format!("\n        {kit}\n");
    let mut i = open + 1;
    loop {
        i = skip_trivia(zon, i);
        if i >= close {
            break;
        }
        // `.name = .{ ... }`, or `.@"name" = .{ ... }`.
        let eq = i + zon[i..]
            .find('=')
            .expect("a dependency is `.name = .{ ... }`");
        let name = zon[i..eq]
            .trim()
            .trim_start_matches('.')
            .trim_start_matches('@')
            .trim_matches('"');
        let entry_open = eq
            + zon[eq..]
                .find('{')
                .expect("a dependency is `.name = .{ ... }`");
        let entry_close = matching_brace(zon, entry_open);
        let entry = &zon[i..=entry_close];
        if name != "fig_kit" && !entry.contains(".path") {
            body.push_str("        ");
            body.push_str(entry);
            body.push_str(",\n");
        }
        i = entry_close + 1;
    }
    body.push_str("    ");
    format!("{}{}{}", &zon[..=open], body, &zon[close..])
}

/// The index of the `}` that closes the `{` at `open`, past strings and
/// comments.
fn matching_brace(s: &str, open: usize) -> usize {
    let b = s.as_bytes();
    let (mut depth, mut i) = (0usize, open);
    while i < b.len() {
        match b[i] {
            b'"' => i = string_end(b, i),
            b'/' if b.get(i + 1) == Some(&b'/') => i = line_end(b, i),
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("a build.zig.zon's braces do not close")
}

/// Past whitespace, commas and comments from `i`.
fn skip_trivia(s: &str, mut i: usize) -> usize {
    let b = s.as_bytes();
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' | b',' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => i = line_end(b, i) + 1,
            _ => break,
        }
    }
    i
}

/// The index of the `"` closing the string opened at `i`.
fn string_end(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() && b[i] != b'"' {
        if b[i] == b'\\' {
            i += 1;
        }
        i += 1;
    }
    i
}

/// The index of the newline ending the line `i` is on, or the end.
fn line_end(b: &[u8], i: usize) -> usize {
    b[i..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |n| i + n)
}

/// Repackage a Zig-produced static archive so Apple's `ld` accepts it.
///
/// Zig's archiver writes members with a zero file mode and without 8-byte
/// alignment. LLD tolerates this, but ld64 errors with "not 8-byte aligned".
/// Extract the members, restore read permissions (so `libtool` can open them),
/// and rebuild the archive with `libtool`, whose output ld64 accepts.
fn repack_archive_for_apple_ld(lib_path: &Path) {
    let work = lib_path
        .parent()
        .expect("library path has a parent")
        .join("repack");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).expect("create repack work dir");

    run(Command::new("ar").arg("x").arg(lib_path).current_dir(&work));

    // Collect the extracted object files, fixing their permissions.
    let mut objects = Vec::new();
    for entry in fs::read_dir(&work).expect("read repack work dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|ext| ext == "o") {
            run(Command::new("chmod").arg("u+rw").arg(&path));
            objects.push(path);
        }
    }
    assert!(
        !objects.is_empty(),
        "no object files extracted from {}",
        lib_path.display()
    );

    let mut libtool = Command::new("libtool");
    libtool
        .arg("-static")
        .arg("-o")
        .arg(lib_path)
        .args(&objects);
    run(&mut libtool);

    let _ = fs::remove_dir_all(&work);
}

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|e| panic!("failed to run {command:?}: {e}"));
    if !status.success() {
        panic!("{command:?} failed with status {status}");
    }
}

fn zig_target_for_cargo_target(target: &str, host: &str) -> Option<&'static str> {
    // Windows MSVC must be handled before the `target == host` shortcut: Zig's
    // native default Windows ABI is GNU-style and emits `__chkstk_ms`, a
    // compiler-rt stack-probe symbol that Zig does not bundle into a static
    // lib, so the MSVC linker can't resolve it. Forcing the msvc ABI makes Zig
    // emit `__chkstk` (provided by the MSVC runtime) and keeps the static lib
    // ABI-compatible with the windows-msvc Rust binary it links into.
    match target {
        "x86_64-pc-windows-msvc" => return Some("x86_64-windows-msvc"),
        "aarch64-pc-windows-msvc" => return Some("aarch64-windows-msvc"),
        _ => {}
    }

    // A macOS build names its target even on the host, with the deployment
    // target Rust links against: a host build without one stamps the
    // archive with the build machine's own macOS version, and whatever links
    // it then refuses to launch on an older one.
    match target {
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => {
            println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
            let arch = if target.starts_with("aarch64") {
                "aarch64"
            } else {
                "x86_64"
            };
            let default = if arch == "aarch64" { "11.0" } else { "10.12" };
            let version =
                env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| default.to_string());
            return Some(Box::leak(
                format!("{arch}-macos.{version}").into_boxed_str(),
            ));
        }
        // iOS too: Zig's own default is newer than the 14.0 the prebuilt
        // archives name (`prebuilt-targets.tsv`).
        "aarch64-apple-ios" | "aarch64-apple-ios-sim" | "x86_64-apple-ios" => {
            println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
            let arch = if target.starts_with("aarch64") {
                "aarch64"
            } else {
                "x86_64"
            };
            let version =
                env::var("IPHONEOS_DEPLOYMENT_TARGET").unwrap_or_else(|_| "14.0".to_string());
            let sim = if target == "aarch64-apple-ios" {
                ""
            } else {
                "-simulator"
            };
            return Some(Box::leak(
                format!("{arch}-ios.{version}{sim}").into_boxed_str(),
            ));
        }
        _ => {}
    }

    if target == host {
        return None;
    }

    match target {
        "aarch64-apple-darwin" => Some("aarch64-macos"),
        "x86_64-apple-darwin" => Some("x86_64-macos"),
        "aarch64-apple-ios" => Some("aarch64-ios"),
        "aarch64-apple-ios-sim" => Some("aarch64-ios-simulator"),
        "x86_64-apple-ios" => Some("x86_64-ios-simulator"),
        "aarch64-pc-windows-gnu" => Some("aarch64-windows-gnu"),
        "x86_64-pc-windows-gnu" => Some("x86_64-windows-gnu"),
        "i686-pc-windows-gnu" => Some("x86-windows-gnu"),
        // Zig bundles no bionic, so the Android archives are built without
        // libc, allocating through `smp_allocator` (`activeAllocator` in
        // c_api.zig), and need no NDK sysroot. Only the two ABIs Android
        // still ships are here.
        "aarch64-linux-android" => Some("aarch64-linux-android"),
        "x86_64-linux-android" => Some("x86_64-linux-android"),
        "aarch64-unknown-linux-gnu" => Some("aarch64-linux-gnu"),
        "aarch64-unknown-linux-musl" => Some("aarch64-linux-musl"),
        "arm-unknown-linux-gnueabi" => Some("arm-linux-gnueabi"),
        "arm-unknown-linux-gnueabihf" => Some("arm-linux-gnueabihf"),
        "arm-unknown-linux-musleabi" => Some("arm-linux-musleabi"),
        "arm-unknown-linux-musleabihf" => Some("arm-linux-musleabihf"),
        "i686-unknown-linux-gnu" => Some("x86-linux-gnu"),
        "i686-unknown-linux-musl" => Some("x86-linux-musl"),
        "powerpc64le-unknown-linux-gnu" => Some("powerpc64le-linux-gnu"),
        "powerpc64le-unknown-linux-musl" => Some("powerpc64le-linux-musl"),
        "riscv64gc-unknown-linux-gnu" => Some("riscv64-linux-gnu"),
        "riscv64gc-unknown-linux-musl" => Some("riscv64-linux-musl"),
        "wasm32-unknown-unknown" => Some("wasm32-freestanding"),
        // Rust's WASI targets are `wasm32-wasip1` (née `wasm32-wasi`) and
        // `wasm32-wasip2`; Zig has no separate preview1/preview2 tag, just
        // `wasi` (preview1). `wasm32-wasip2` maps to the same Zig target: fig's
        // C surface does no I/O (no filesystem, no host calls) — it only
        // touches its own linear memory — so a preview1-ABI static lib links
        // and runs correctly under Rust's wasip2 sysroot, which supplies the
        // actual environment/component surface itself and never needs
        // anything from this library's own (nonexistent) WASI imports.
        // Verified by linking+running `fig/examples/wasm_smoke.rs` for both
        // targets under wasmtime; revisit if fig ever grows real I/O.
        "wasm32-wasip1" => Some("wasm32-wasi"),
        "wasm32-wasip2" => Some("wasm32-wasi"),
        "x86_64-unknown-linux-gnu" => Some("x86_64-linux-gnu"),
        "x86_64-unknown-linux-musl" => Some("x86_64-linux-musl"),
        _ => panic!(
            "unsupported Rust target `{target}` for fig's bundled Zig static library; \
             add a Cargo-to-Zig target mapping in fig-language-build"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::with_kit_at;

    #[test]
    fn the_kit_is_named_and_path_dependencies_dropped() {
        let zon = r#".{
    .name = .fig_plist,
    .dependencies = .{
        .fig_kit = .{ .path = "../../kit" },
        // Its tests print what they read as JSON: "}".
        .fig_json = .{ .path = "../json" },
        .@"other" = .{ .url = "https://example.com/x.tar.gz", .hash = "abc" },
    },
    .paths = .{ "src" },
}"#;
        let out = with_kit_at(zon, "../kit");
        assert!(
            out.contains(r#".fig_kit = .{ .path = "../kit" },"#),
            "{out}"
        );
        assert!(
            !out.contains("../json") && !out.contains("../../kit"),
            "{out}"
        );
        assert!(
            out.contains(
                r#".@"other" = .{ .url = "https://example.com/x.tar.gz", .hash = "abc" },"#
            ),
            "{out}"
        );
        assert!(out.contains(r#".paths = .{ "src" },"#), "{out}");
    }

    #[test]
    fn a_package_with_no_dependencies_gets_the_kit() {
        let out = with_kit_at(".{\n    .name = .x,\n}\n", "../kit");
        assert!(
            out.contains(r#".dependencies = .{ .fig_kit = .{ .path = "../kit" }, },"#),
            "{out}"
        );
    }
}
