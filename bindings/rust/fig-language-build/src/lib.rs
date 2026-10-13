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
    let prefix =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("zig-prefix");

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
        .current_dir(&package)
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
    for input in ["build.zig", "build.zig.zon", "src"] {
        println!("cargo:rerun-if-changed={}", package.join(input).display());
    }
    // A language of fig's own builds with the kit beside it in fig's tree
    // (`languages/<name>/` and `kit/`); one written elsewhere fetches it.
    let kit = package.join("../../kit");
    if kit.join("build.zig").is_file() {
        println!("cargo:rerun-if-changed={}", kit.display());
    }
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
