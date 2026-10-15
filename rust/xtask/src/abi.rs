//! `abi-check`: hold the C ABI's three sides — the header
//! (bindings/c/include/fig.h), the Rust that implements it (rust/fig-capi,
//! rust/fig-lang-export) and the library built from that Rust — to agreeing.
//!
//! The source checks, which need no build:
//!   * fig.h's `FIG_VERSION_*` against the workspace's version, so the header
//!     a caller compiles against cannot drift from the package;
//!   * fig.h's `FIG_ABI_VERSION` against fig-capi's `ABI_VERSION`, what
//!     `fig_abi_version()` reports, and `FIG_LANGUAGE_VTABLE_VERSION` against
//!     the `VTABLE_VERSION` both fig-capi's registry (which refuses any other)
//!     and fig-lang-export's vtables (which write it) are compiled with;
//!   * no surface — fig.h, the TypeScript binding, the Rust binding — states a
//!     fixed format number again: every format is a registered language's
//!     handle (6.0);
//!   * fig.h's `FIG_EXT_*` (name and value), the TypeScript `ExtKind` (name and
//!     value) and the Rust binding's `ExtKind` (name; its value is its order)
//!     against the core's `ExtKind` in fig-kit, whose ordinal is the ABI value.
//!     Once fig.h and both bindings stopped at `number_special` for a release
//!     while `fig_node_extended` already returned the plist kinds after it.
//!
//! Then the build: libfig (fig-capi as a staticlib, no language bundled) and
//! the yaml and json languages' own libraries (fig-lang-export), copied under
//! target/xtask/. The `fig_*` symbols libfig.a defines (`nm`, with macOS's
//! leading underscore dropped) are diffed both ways against the `fig_x(`
//! tokens on fig.h's non-comment lines: an export without a declaration is a
//! function no caller can find, a declaration without an export a dangling
//! prototype. The wasm-only exports (fig-capi's `fig_host_*`, fig-kit-capi's
//! `fig_kit_alloc`/`fig_kit_free`) are not in a native build, so they never
//! reach the diff. Signatures are not compared: C has no name mangling, so a
//! parameter change links fine — `semver-check` compares declarations.
//!
//! Then the probes, with the system's C and C++ compilers (`$CC`/`$CXX`, else
//! `cc`/`c++`), each linked with the native libraries rustc says its
//! staticlibs need: abi_probe.c and abi_probe.cpp prove fig.h parses and
//! links as C and as C++; abi_runtime_probe.c is run, a language written in C
//! whose every runtime-section struct the Rust side reads, which is the
//! layout agreement the symbol diff cannot see; lang_archive_probe.c is run,
//! linking the two languages' libraries beside a libfig that names none. A
//! platform without a C++ compiler skips that one probe with a note; the rest
//! are required.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::semver::{header_version, macro_int, workspace_version};

const HEADER: &str = "bindings/c/include/fig.h";

pub fn run(args: &[String]) -> crate::Result {
    if let Some(extra) = args.first() {
        return Err(format!("takes no arguments (got `{extra}`)"));
    }
    let root = crate::root();
    let header = read(&root, HEADER)?;
    let mut fail = Fail::default();

    // --- Versions. ---
    let want_version = workspace_version(&root)?;
    match header_version(&header) {
        Some(v) if v == want_version => {}
        Some(v) => fail.line(format!(
            "version drift: fig.h is {v} but Cargo.toml is {want_version} (run `cargo xtask version-sync`)"
        )),
        None => fail.line("fig.h has no FIG_VERSION_MAJOR/MINOR/PATCH macros".into()),
    }
    let abi = rust_const(&root, "rust/fig-capi/src/lib.rs", "ABI_VERSION")?;
    match macro_int(&header, "FIG_ABI_VERSION") {
        Some(h) if h == abi => {}
        Some(h) => fail.line(format!(
            "ABI-version drift: fig.h FIG_ABI_VERSION is {h} but rust/fig-capi's ABI_VERSION is {abi} (update the macro or the constant)"
        )),
        None => fail.line("fig.h has no FIG_ABI_VERSION macro".into()),
    }
    let header_vt = macro_int(&header, "FIG_LANGUAGE_VTABLE_VERSION");
    for file in [
        "rust/fig-capi/src/language.rs",
        "rust/fig-lang-export/src/c.rs",
    ] {
        let vt = rust_const(&root, file, "VTABLE_VERSION")?;
        match header_vt {
            Some(h) if h == vt => {}
            Some(h) => fail.line(format!(
                "vtable-version drift: fig.h FIG_LANGUAGE_VTABLE_VERSION is {h} but {file}'s VTABLE_VERSION is {vt}"
            )),
            None => fail.line("fig.h has no FIG_LANGUAGE_VTABLE_VERSION macro".into()),
        }
    }

    // --- No fixed format numbers. ---
    let ts = read(&root, "bindings/typescript/src/types.ts")?;
    let rust_lib = read(&root, "bindings/rust/fig/src/lib.rs")?;
    for (surface, src, needle) in [
        ("fig.h", &header, "typedef enum FigFormat"),
        ("fig.h", &header, "#define FIG_FORMAT_RUNTIME_BASE"),
        ("TypeScript", &ts, "export const Format = {"),
        ("Rust", &rust_lib, "pub enum Format"),
    ] {
        if src.contains(needle) {
            fail.line(format!(
                "{surface} states `{needle}`: formats are registered handles, with no fixed numbers"
            ));
        }
    }

    // --- Extended-scalar kinds. ---
    let core = core_ext_kinds(&read(&root, "rust/fig-kit/src/ast/mod.rs")?)?;
    let rust_value = read(&root, "bindings/rust/fig/src/value.rs")?;
    for (surface, text, decl, style) in [
        ("fig.h", &header, "typedef enum FigExtKind", Style::CMacro),
        (
            "TypeScript ExtKind",
            &ts,
            "export const ExtKind = {",
            Style::PascalValued,
        ),
        (
            "Rust ExtKind",
            &rust_value,
            "pub enum ExtKind",
            Style::PascalNamed,
        ),
    ] {
        let found = enumerators(text, decl).map_err(|e| format!("{surface}: {e}"))?;
        check_ext_kinds(&mut fail, surface, &core, &found, style);
    }

    // --- Build, then the symbol diff. ---
    let out = target_dir(&root).join("xtask");
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let (libfig, mut native) = build(&root, &["-p", "fig-capi"], &out.join("libfig.a"))?;
    let mut lang_libs = Vec::new();
    for name in ["yaml", "json"] {
        let lib = out.join(format!("libfig_{name}.a"));
        let (_, libs) = build(&root, &["-p", "fig-lang-export", "--features", name], &lib)?;
        // Every language build writes the one libfig_lang_export.a; this is
        // the one asked for only if it registers the language asked for.
        let register = format!("fig_{name}_register");
        if !exported(&lib)?.contains(&register) {
            return Err(format!("{} does not define {register}", lib.display()));
        }
        for l in libs {
            if !native.contains(&l) {
                native.push(l);
            }
        }
        lang_libs.push(lib);
    }
    let exported = exported(&libfig)?;
    let declared = declared(&header);
    for name in exported.iter().filter(|n| !declared.contains(n)) {
        fail.line(format!(
            "exported by libfig.a but NOT declared in fig.h: {name}"
        ));
    }
    for name in declared.iter().filter(|n| !exported.contains(n)) {
        fail.line(format!(
            "declared in fig.h but NOT exported by libfig.a: {name}"
        ));
    }
    if fail.failed {
        return Err("fig.h, the Rust and libfig.a disagree (above)".into());
    }
    eprintln!(
        "abi-check: symbol diff OK ({} symbols), version {want_version}, ABI v{abi}",
        exported.len()
    );
    eprintln!("abi-check: no surface states a fixed format number");
    eprintln!(
        "abi-check: FIG_EXT_* enumerators OK ({} kinds) — fig.h, the TypeScript and Rust `ExtKind` enums all match the core's ExtKind",
        core.len()
    );

    // --- The probes. ---
    let include = root.join("bindings/c/include");
    let inc = format!("-I{}", include.display());
    let cc = compiler("CC", "cc");
    let cxx = compiler("CXX", "c++");
    let link = |lib: &[&Path]| -> Vec<String> {
        lib.iter()
            .map(|p| p.display().to_string())
            .chain(native.iter().cloned())
            .collect()
    };

    probe(
        &root,
        &cc,
        "tools/abi_probe.c",
        &[&inc],
        &link(&[&libfig]),
        &out.join("abi_probe_c"),
        false,
    )?;
    eprintln!("abi-check: abi_probe.c compiles and links (fig.h as C)");
    if have_compiler(&cxx) {
        probe(
            &root,
            &cxx,
            "tools/abi_probe.cpp",
            &[&inc],
            &link(&[&libfig]),
            &out.join("abi_probe_cpp"),
            false,
        )?;
        eprintln!("abi-check: abi_probe.cpp compiles and links (fig.h as C++)");
    } else {
        eprintln!(
            "abi-check: note: no C++ compiler (`{}`) — skipping abi_probe.cpp",
            cxx.join(" ")
        );
    }
    let runtime = out.join("abi_runtime_probe");
    probe(
        &root,
        &cc,
        "tools/abi_runtime_probe.c",
        &[&inc],
        &link(&[&libfig]),
        &runtime,
        true,
    )?;
    eprintln!(
        "abi-check: abi_runtime_probe ran OK (a C-hosted language through the runtime structs)"
    );
    let yaml_inc = format!("-I{}", root.join("languages/yaml/include").display());
    let json_inc = format!("-I{}", root.join("languages/json/include").display());
    let archives: Vec<&Path> = lang_libs
        .iter()
        .map(PathBuf::as_path)
        .chain([libfig.as_path()])
        .collect();
    probe(
        &root,
        &cc,
        "tools/lang_archive_probe.c",
        &[&inc, &yaml_inc, &json_inc],
        &link(&archives),
        &out.join("lang_archive_probe"),
        true,
    )?;
    eprintln!(
        "abi-check: lang_archive_probe ran OK (libfig_yaml.a and libfig_json.a beside libfig.a)"
    );
    Ok(())
}

/// The failure lines, headed by one `abi-check: FAIL` as the old tool printed
/// them, so a CI log reads the same.
#[derive(Default)]
struct Fail {
    failed: bool,
}

impl Fail {
    fn line(&mut self, message: String) {
        if !self.failed {
            eprintln!("abi-check: FAIL");
            self.failed = true;
        }
        eprintln!("  {message}");
    }
}

fn read(root: &Path, path: &str) -> crate::Result<String> {
    std::fs::read_to_string(root.join(path)).map_err(|e| format!("cannot read {path}: {e}"))
}

/// The value of `const <name>: u32 = <int>;` in a Rust source file: the
/// constants are read from source, as the xtask links none of the crates.
fn rust_const(root: &Path, path: &str, name: &str) -> crate::Result<u32> {
    let text = read(root, path)?;
    let needle = format!("const {name}: u32 =");
    text.lines()
        .find_map(|line| {
            let rest = line.split_once(&needle)?.1;
            rest.split(';').next()?.trim().parse().ok()
        })
        .ok_or_else(|| format!("{path} has no `{needle} <int>;`"))
}

/// Where cargo builds: `$CARGO_TARGET_DIR`, else the workspace's target/.
fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |d| root.join(d))
}

/// `cargo rustc <package> --release --crate-type staticlib`, the archive it
/// writes copied to `dest`, with the native libraries rustc says a link of it
/// needs (`-lSystem -lc -lm` on macOS; on Linux `-lgcc_s -lpthread -ldl …`).
/// Cargo replays the note on a fresh build, so it is there every time.
fn build(root: &Path, package: &[&str], dest: &Path) -> crate::Result<(PathBuf, Vec<String>)> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // `--color never` because the note is read back as text: CI sets
    // CARGO_TERM_COLOR=always, and rustc's colored note ends `-lc\x1b[0m`.
    let mut args = vec!["rustc", "--color", "never"];
    args.extend_from_slice(package);
    args.extend_from_slice(&[
        "--release",
        "--crate-type",
        "staticlib",
        "--",
        "--print",
        "native-static-libs",
    ]);
    eprintln!("· cargo {}", args.join(" "));
    let output = Command::new(&cargo)
        .args(&args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run {cargo}: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        eprint!("{stderr}");
        return Err(format!(
            "`cargo {}` failed ({})",
            args.join(" "),
            output.status
        ));
    }
    let native = stderr
        .lines()
        .find_map(|l| l.split_once("native-static-libs:").map(|(_, libs)| libs))
        // macOS's `cc` links libSystem itself, and ld warns at it twice.
        .map(|libs| {
            libs.split_whitespace()
                .filter(|l| *l != "-lSystem")
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let crate_name = package[1].replace('-', "_");
    let built = target_dir(root)
        .join("release")
        .join(format!("lib{crate_name}.a"));
    std::fs::copy(&built, dest)
        .map_err(|e| format!("cannot copy {} to {}: {e}", built.display(), dest.display()))?;
    Ok((dest.to_path_buf(), native))
}

/// The `fig_*` functions an archive defines, sorted: the `T` lines of `nm -g`
/// (`$NM` if set), Mach-O's leading underscore dropped.
fn exported(archive: &Path) -> crate::Result<Vec<String>> {
    let nm = std::env::var("NM").unwrap_or_else(|_| "nm".into());
    let output = Command::new(&nm)
        .arg("-g")
        .arg(archive)
        .output()
        .map_err(|e| format!("cannot run {nm}: {e}"))?;
    // Not a failure on its own: a Rust staticlib carries std's objects as the
    // toolchain's LLVM bitcode, which an older system `nm` (Xcode's) cannot
    // read and exits 1 over, after listing every object it could — fig's
    // among them. An `nm` that listed nothing has failed for real.
    if !output.status.success() && output.stdout.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return Err(format!(
            "`{nm} -g {}` failed ({})",
            archive.display(),
            output.status
        ));
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let [.., kind, name] = tokens[..] else {
                return None;
            };
            let name = name
                .strip_prefix('_')
                .filter(|n| n.starts_with("fig_"))
                .unwrap_or(name);
            (kind == "T" && name.starts_with("fig_")).then(|| name.to_string())
        })
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'
}

/// The header's prototypes: each `fig_<name>(` token (the `(` tells a
/// declaration from a prose mention) on a line that is not a `//` comment.
fn declared(header: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in header.lines().filter(|l| !l.trim_start().starts_with("//")) {
        let b = line.as_bytes();
        let mut i = 0;
        while let Some(off) = line[i..].find("fig_") {
            let pos = i + off;
            let mut end = pos + 4;
            while end < b.len() && is_name_char(b[end]) {
                end += 1;
            }
            // Not the tail of a longer identifier, and called.
            if !(pos > 0 && is_name_char(b[pos - 1])) && b.get(end) == Some(&b'(') {
                names.push(line[pos..end].to_string());
            }
            i = end;
        }
    }
    names.sort();
    names.dedup();
    names
}

/// How a surface spells an `ExtKind` member, and whether it states a value.
#[derive(Clone, Copy, PartialEq)]
enum Style {
    /// `FIG_EXT_OFFSET_DATETIME = 0` — fig.h.
    CMacro,
    /// `OffsetDateTime: 0` — the TypeScript `ExtKind` object.
    PascalValued,
    /// `OffsetDateTime` — the Rust binding, whose value is its order.
    PascalNamed,
}

/// A member's spelling on a surface, from the core's snake_case name:
/// `FIG_EXT_OFFSET_DATETIME` in C; `OffsetDateTime` in the bindings, which
/// upper-camel-case at each underscore and spell `datetime` `DateTime`.
fn ext_kind_name(snake: &str, style: Style) -> String {
    match style {
        Style::CMacro => format!("FIG_EXT_{}", snake.to_ascii_uppercase()),
        Style::PascalValued | Style::PascalNamed => snake
            .split('_')
            .map(|word| match word {
                "datetime" => "DateTime".to_string(),
                w => w[..1].to_ascii_uppercase() + &w[1..],
            })
            .collect(),
    }
}

/// The core's `ExtKind` (fig-kit's `ast` module) as snake_case names with
/// their ABI values: an explicit discriminant where one is written, else one
/// past the previous, from 0.
fn core_ext_kinds(source: &str) -> crate::Result<Vec<(String, i64)>> {
    let mut next = 0;
    let mut kinds = Vec::new();
    for (name, value) in enumerators(source, "pub enum ExtKind")
        .map_err(|e| format!("rust/fig-kit/src/ast/mod.rs: {e}"))?
    {
        let value = value.unwrap_or(next);
        next = value + 1;
        let mut snake = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                snake.push('_');
            }
            snake.push(c.to_ascii_lowercase());
        }
        kinds.push((snake, value));
    }
    Ok(kinds)
}

/// Every core kind declared on the surface, with its value where the surface
/// states one; the surface declaring nothing the core lacks.
fn check_ext_kinds(
    fail: &mut Fail,
    surface: &str,
    core: &[(String, i64)],
    found: &[(String, Option<i64>)],
    style: Style,
) {
    for (snake, value) in core {
        let want = ext_kind_name(snake, style);
        match found.iter().find(|(n, _)| *n == want) {
            None => fail.line(format!(
                "{surface}: missing enumerator: the core's ExtKind has '{snake}' (value {value}) but {want} is not declared"
            )),
            Some(_) if style == Style::PascalNamed => {}
            Some((_, Some(got))) if got != value => fail.line(format!(
                "{surface}: ext-kind value drift: {want} = {got} but the core's ExtKind gives '{snake}' the value {value}"
            )),
            Some((_, None)) => fail.line(format!(
                "{surface}: {want} states no value, but the core's ExtKind gives '{snake}' the value {value}"
            )),
            Some(_) => {}
        }
    }
    for (name, _) in found {
        if !core
            .iter()
            .any(|(snake, _)| ext_kind_name(snake, style) == *name)
        {
            fail.line(format!(
                "{surface}: unknown enumerator: {name} matches no core ExtKind member"
            ));
        }
    }
    if style == Style::PascalNamed {
        // The binding's value is its order, so its order is checked too.
        let order: Vec<String> = core.iter().map(|(s, _)| ext_kind_name(s, style)).collect();
        let got: Vec<&String> = found.iter().map(|(n, _)| n).collect();
        if got.len() == order.len() && got.iter().zip(&order).any(|(g, o)| *g != o) {
            fail.line(format!(
                "{surface}: the variants are not in the core's order, which is their ABI value"
            ));
        }
    }
}

/// The enumerators of the enum declared at `decl` in `text`, in order:
/// `Name = value` (C, Rust), `Name: value` (a TypeScript `as const`
/// object) or a bare `Name`. Comments and `#[…]` attribute lines are removed
/// on the way to the closing brace, not after: the prose in them holds commas
/// and braces (`{@link …}`) that would otherwise split a member or end the
/// enum early.
fn enumerators(text: &str, decl: &str) -> crate::Result<Vec<(String, Option<i64>)>> {
    let at = text.find(decl).ok_or_else(|| format!("no `{decl}`"))?;
    let open = text[at..]
        .find('{')
        .ok_or_else(|| format!("`{decl}` has no body"))?
        + at;
    let mut code = String::new();
    let mut rest = &text[open + 1..];
    let mut line_start = true;
    loop {
        if rest.is_empty() {
            return Err(format!("`{decl}` is not closed"));
        }
        if rest.starts_with("//")
            || (line_start && rest.trim_start_matches([' ', '\t']).starts_with("#["))
        {
            rest = rest.find('\n').map_or("", |n| &rest[n..]);
            continue;
        }
        if let Some(after) = rest.strip_prefix("/*") {
            rest = &after[after
                .find("*/")
                .ok_or_else(|| format!("`{decl}` has an unclosed comment"))?
                + 2..];
            continue;
        }
        let c = rest.chars().next().expect("nonempty");
        rest = &rest[c.len_utf8()..];
        if c == '}' {
            break;
        }
        line_start = c == '\n';
        if c != '\n' && c != '\r' {
            code.push(c);
        }
    }
    code.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| match item.split_once(['=', ':']) {
            Some((name, value)) => value
                .trim()
                .parse()
                .map(|v| (name.trim().to_string(), Some(v)))
                .map_err(|_| format!("`{decl}`: `{item}` has no decimal value")),
            None => Ok((item.to_string(), None)),
        })
        .collect()
}

/// `$<var>` split into a program and its leading arguments (`ccache cc`),
/// else `default`.
fn compiler(var: &str, default: &str) -> Vec<String> {
    let value = std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.into());
    value.split_whitespace().map(String::from).collect()
}

fn have_compiler(cmd: &[String]) -> bool {
    Command::new(&cmd[0])
        .args(&cmd[1..])
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Compile and link `source` into `exe`, and run it when `run` is set.
fn probe(
    root: &Path,
    compiler: &[String],
    source: &str,
    includes: &[&str],
    link: &[String],
    exe: &Path,
    run: bool,
) -> crate::Result {
    let exe_s = exe.display().to_string();
    let mut args: Vec<&str> = compiler[1..].iter().map(String::as_str).collect();
    args.push(source);
    args.extend_from_slice(includes);
    args.extend(link.iter().map(String::as_str));
    args.extend_from_slice(&["-o", &exe_s]);
    crate::run(root, &compiler[0], &args).map_err(|e| format!("{source}: {e}"))?;
    if run {
        crate::run(root, &exe_s, &[]).map_err(|e| format!("{source}: {e}"))?;
    }
    Ok(())
}
