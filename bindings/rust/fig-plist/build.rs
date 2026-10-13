// The build script every language crate (`fig-<name>`) shares, written by
// `scripts/gen-language-crates.sh` from `scripts/language-crate/build.rs`:
// edit that one, not a copy. It compiles the language's own static library,
// `libfig_<name>.a` (`zig build c-language-<name>`), which links beside
// fig-sys's `libfig.a`; the name is the crate's, less `fig-`.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // docs.rs has no Zig toolchain; `cargo doc` never links the library.
    if env::var_os("DOCS_RS").is_some() {
        return;
    }
    let crate_name = env::var("CARGO_PKG_NAME").expect("Cargo sets CARGO_PKG_NAME");
    let name = crate_name
        .strip_prefix("fig-")
        .expect("a language crate is named fig-<name>");
    let cargo_target = env::var("TARGET").expect("Cargo should set TARGET");
    let cargo_host = env::var("HOST").expect("Cargo should set HOST");
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source_root = zig_source_root(&manifest_dir);
    let prefix = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("zig-prefix");

    let mut command = Command::new("zig");
    command
        .arg("build")
        .arg(format!("c-language-{name}"))
        .arg("-Doptimize=ReleaseFast")
        .arg("-Dstrip=true")
        // The portable baseline CPU, as fig-sys's build pins it.
        .arg("-Dcpu=baseline")
        .arg("-Dhelper-wire=false");
    if let Some(zig_target) = zig_target_for_cargo_target(&cargo_target, &cargo_host) {
        command.arg(format!("-Dtarget={zig_target}"));
    }
    let status = command
        .arg("--prefix")
        .arg(&prefix)
        .current_dir(&source_root)
        .status()
        .unwrap_or_else(|e| panic!("{crate_name}: compiling the language's library needs Zig 0.17+ (https://ziglang.org/download/), and running `zig` failed: {e}"));
    if !status.success() {
        panic!("`zig build c-language-{name}` failed with status {status}");
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
    println!("cargo:rerun-if-env-changed=FIG_ZIG_ROOT");
    println!(
        "cargo:rerun-if-changed={}",
        source_root.join("src").display()
    );
}

fn zig_source_root(manifest_dir: &Path) -> PathBuf {
    if let Some(dir) = env::var_os("FIG_ZIG_ROOT") {
        let dir = PathBuf::from(dir);
        assert!(
            is_fig_zig_root(&dir),
            "FIG_ZIG_ROOT={} is not a fig Zig source tree (no build.zig + src/c_api.zig)",
            dir.display()
        );
        return dir;
    }

    for ancestor in manifest_dir.ancestors() {
        if is_fig_zig_root(ancestor) {
            return ancestor.to_path_buf();
        }
    }

    let vendored = manifest_dir.join("zig");
    if is_fig_zig_root(&vendored) {
        return vendored;
    }

    panic!(
        "could not locate fig's Zig source. In a checkout it is found by walking up \
         from {}; in a published crate it is vendored at ./zig — run `zig build vendor-rust` \
         to populate it, or set FIG_ZIG_ROOT to a fig source tree.",
        manifest_dir.display()
    );
}

/// Whether `dir` is the root of fig's Zig source: enough of a fingerprint that we
/// won't mistake an unrelated `build.zig` for it.
fn is_fig_zig_root(dir: &Path) -> bool {
    dir.join("build.zig").is_file()
        && dir.join("build.zig.zon").is_file()
        && dir.join("src/c_api.zig").is_file()
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
             add a Cargo-to-Zig target mapping in bindings/rust/build.rs"
        ),
    }
}
