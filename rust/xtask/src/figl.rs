//! `sync-figl` / `check-figl`: keep the repository's hand-authored `.figl`
//! files and the GitHub Actions workflows generated from them in step, by
//! running fig itself (`fig get -o yaml <src>`) rather than re-implementing
//! any parsing or printing here. It dogfoods the conversion path a user of
//! `fig convert` / `fig get` takes.
//!
//! The `.figl` files are the source of truth: comments, structure and
//! intent live there, and the generated files are a build artifact of them.
//! A leading comment in each source rides fig's cross-format comment
//! preservation into the generated file, so it (not this task) is what tells
//! a reader not to hand-edit the output.
//!
//! `sync-figl` regenerates every destination that differs; `check-figl`
//! writes nothing and fails if any is stale or missing, which is what CI and
//! the pre-commit hook run. The `fig` they run is this workspace's, built
//! first.

use std::path::Path;
use std::process::Command;

/// Each generated file and the `.figl` it is generated from. Add a line
/// here when a new generated file gets a `.figl` source.
const MAPPINGS: &[(&str, &str)] = &[
    ("figl/ci.figl", ".github/workflows/ci.yml"),
    ("figl/fuzz.figl", ".github/workflows/fuzz.yml"),
    ("figl/homebrew.figl", ".github/workflows/homebrew.yml"),
    ("figl/release-binaries.figl", ".github/workflows/release-binaries.yml"),
    ("figl/release.figl", ".github/workflows/release.yml"),
    ("figl/release-npm.figl", ".github/workflows/release-npm.yml"),
    ("figl/release-npm-wasi.figl", ".github/workflows/release-npm-wasi.yml"),
];

pub fn run(check: bool) -> crate::Result {
    let root = crate::root();
    crate::run(&root, "cargo", &["build", "--quiet", "-p", "fig-cli", "--bin", "fig"])?;
    let fig = root.join("target/debug/fig");
    let mut failed = false;
    for &(src, dest) in MAPPINGS {
        let generated = match generate(&fig, &root.join(src)) {
            Ok(g) => g,
            Err(message) => {
                eprintln!("sync-figl: {message}");
                failed = true;
                continue;
            }
        };
        let dest_path = root.join(dest);
        let existing = std::fs::read(&dest_path).ok();
        let up_to_date = existing.as_deref() == Some(&generated[..]);
        if check {
            if up_to_date {
                eprintln!("sync-figl: ok      {dest}");
            } else {
                let what = if existing.is_none() { "MISSING" } else { "STALE  " };
                eprintln!("sync-figl: {what} {dest} (run `cargo xtask sync-figl`)");
                failed = true;
            }
            continue;
        }
        if up_to_date {
            eprintln!("sync-figl: up to date  {dest}");
            continue;
        }
        std::fs::write(&dest_path, &generated).map_err(|e| format!("cannot write {dest}: {e}"))?;
        eprintln!("sync-figl: updated     {dest}");
    }
    if failed { Err("a generated file is out of step with its .figl source".into()) } else { Ok(()) }
}

/// The YAML `fig get -o yaml` prints for `src`.
fn generate(fig: &Path, src: &Path) -> crate::Result<Vec<u8>> {
    let shown = src.display();
    let out = Command::new(fig)
        .args(["get", "-o", "yaml"])
        .arg(src)
        .output()
        .map_err(|e| format!("failed to run `fig get -o yaml {shown}`: {e}"))?;
    if !out.status.success() {
        return Err(format!("`fig get -o yaml {shown}` {}:\n{}", out.status, String::from_utf8_lossy(&out.stderr)));
    }
    Ok(out.stdout)
}
