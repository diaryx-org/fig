//! `vendor` / `vendor-check`: copy the repo-root license texts and README into
//! the Rust crates, so each published crate carries them.
//!
//! crates.io takes the SPDX `license` field, but a published crate should also
//! carry the license *text*, and the `fig` crate the README crates.io renders
//! on its page. A crate published to crates.io cannot reach outside its own
//! directory, so before packaging they are copied into each crate's directory
//! (git-ignored there; each crate's `include` force-adds them to the tarball).
//! `std::fs::copy` rather than a shell `cp`, so it runs the same on Windows.
//!
//! `vendor-check` verifies that every path the copy reads from or writes into
//! still exists, copying nothing. `cargo xtask ci` runs it, because the copy
//! itself only ever runs at publish time: renaming a file out from under this
//! list is invisible until a release workflow dies on it, which is exactly how
//! `fig.md` -> `README.md` once reached CI.

use std::path::Path;

/// The dual license, its one copy at the repository root, copied into every
/// published crate's directory.
const LICENSE_FILES: &[&str] = &["LICENSE-MIT", "LICENSE-APACHE"];

/// The crates that carry the licenses, before the languages: the binding and
/// its derive macros, the kit, and the core. Each language's crate
/// (`languages/<name>/`) follows, from [`crate_dirs`].
const FIXED_CRATE_DIRS: &[&str] = &["bindings/rust/fig", "bindings/rust/fig-macros", "rust/fig-kit", "rust/fig-core"];

/// The repo-root README, which the `fig` crate carries as its own so crates.io
/// has a page to render. Named once here because it is the file this task is
/// most exposed to: it is the only source path that isn't build input, so a
/// rename of it breaks nothing until publish day.
const README_SRC: &str = "README.md";

/// The one crate the README is copied into, as `README.md`. Each language's
/// crate has a README of its own.
const README_CRATE: &str = "bindings/rust/fig";

pub fn run(check: bool) -> crate::Result {
    let root = crate::root();
    let crate_dirs = crate_dirs(&root)?;
    if check {
        return check_sources(&root, &crate_dirs);
    }

    for dir in &crate_dirs {
        let dir = root.join(dir);
        if !dir.is_dir() {
            return Err(format!("no crate directory {}", dir.display()));
        }
        for name in LICENSE_FILES {
            copy(&root.join(name), &dir.join(name))?;
        }
    }
    copy(&root.join(README_SRC), &root.join(README_CRATE).join("README.md"))?;

    eprintln!("vendor: copied {} licenses x{} crates + 1 readme", LICENSE_FILES.len(), crate_dirs.len());
    Ok(())
}

/// [`FIXED_CRATE_DIRS`], then `languages/<name>` for every language fig ships:
/// each directory under `languages/` with a `Cargo.toml`, the same set the
/// workspace's `languages/*` member glob takes, in name order.
fn crate_dirs(root: &Path) -> crate::Result<Vec<String>> {
    let dir = root.join("languages");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
        if entry.path().join("Cargo.toml").is_file() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    let mut out: Vec<String> = FIXED_CRATE_DIRS.iter().map(|s| s.to_string()).collect();
    out.extend(names.iter().map(|n| format!("languages/{n}")));
    Ok(out)
}

fn copy(from: &Path, to: &Path) -> crate::Result {
    std::fs::copy(from, to).map(drop).map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
}

/// Verify every path the copy reads or writes into, without writing anything:
/// every crate directory, the license texts, and the crate README.
fn check_sources(root: &Path, crate_dirs: &[String]) -> crate::Result {
    let mut missing = 0;
    for dir in crate_dirs {
        missing += report(root, dir, "crate dir");
    }
    for name in LICENSE_FILES {
        missing += report(root, name, "license");
    }
    missing += report(root, README_SRC, "crate README");

    if missing != 0 {
        return Err(format!("{missing} missing path(s) — a publish would fail on the first one"));
    }
    eprintln!("vendor: OK ({} crate dirs + {} licenses + README all present)", crate_dirs.len(), LICENSE_FILES.len());
    Ok(())
}

/// `1` if `rel` is absent (having said so), `0` if it is there.
fn report(root: &Path, rel: &str, what: &str) -> usize {
    if root.join(rel).exists() {
        0
    } else {
        eprintln!("vendor: MISSING {what}: {rel}");
        1
    }
}
