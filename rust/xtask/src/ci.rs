//! `ci`: the one pre-release gate. Everything a change has to pass before it
//! is called done, and what `dx release` runs after a bump (`verify` in
//! `.config/release.toml`):
//!
//! - the workspace's tests, with every feature on and then with the crates a
//!   program takes as it takes them (default features);
//! - the release guards: `version-check`, `vendor-check`, `check-figl`,
//!   `abi-check` and `semver-check`;
//! - the built CLI end to end (`tools/cli-*-check.sh`): its usage errors,
//!   `fig patch` behind jq, and the helper runner;
//! - every language's edit fixtures through the Rust editor, behind
//!   fig-conformance's adapter (`fig-conform`);
//! - the TypeScript binding's and the WASI package's suites, and the edit
//!   fixtures again through the binding.
//!
//! The suites that need a toolchain beyond cargo — Node for the fixtures,
//! Node 24 and an installed `node_modules` for the npm packages — skip with
//! a note where it is missing, so a contributor with Rust alone still gets a
//! useful gate. CI installs them (the `typescript` and `wasi` jobs).

use std::path::Path;
use std::process::Command;

use crate::{have, root};

pub fn run(args: &[String]) -> crate::Result {
    if let Some(a) = args.first() {
        return Err(format!("`ci` takes no arguments, not `{a}`"));
    }
    let root = root();
    let r = root.as_path();

    crate::run(r, "cargo", &["test", "--workspace", "--all-features", "--quiet"])?;
    crate::run(r, "cargo", &["test", "--quiet", "-p", "fig-kit", "-p", "fig-core", "-p", "fig-cli", "-p", "fig-capi", "-p", "fig-conform"])?;

    crate::version::run(true)?;
    crate::vendor::run(true)?;
    crate::figl::run(true)?;
    crate::abi::run(&[])?;
    crate::semver::run(&[])?;

    crate::run(r, "cargo", &["build", "--quiet", "-p", "fig-cli", "--bin", "fig"])?;
    let fig = r.join("target/debug/fig");
    let fig = fig.to_str().ok_or("the target directory's path is not UTF-8")?;
    for script in ["tools/cli-args-check.sh", "tools/cli-patch-check.sh", "tools/cli-lang-check.sh"] {
        crate::run(r, "sh", &[script, fig])?;
    }

    if have("node") {
        crate::run(r, "cargo", &["build", "--quiet", "--release", "-p", "fig-conform"])?;
        crate::run(r, "node", &["tools/edit-fixtures.mjs", "--command", "target/release/fig-conform"])?;
    } else {
        eprintln!("edit fixtures: node not found — skipping (install Node.js to run them).");
    }

    if let Some(why) = npm_missing(&r.join("bindings/typescript")) {
        eprintln!("typescript tests: {why}");
    } else {
        let ts = r.join("bindings/typescript");
        crate::run(&ts, "npm", &["run", "build"])?;
        crate::run(&ts, "npm", &["test"])?;
        crate::run(&ts, "node", &["--conditions=fig-source", "../../tools/edit-fixtures.mjs"])?;
    }
    if let Some(why) = npm_missing(&r.join("bindings/wasi")) {
        eprintln!("wasi tests: {why}");
    } else {
        let wasi = r.join("bindings/wasi");
        crate::run(&wasi, "npm", &["run", "build"])?;
        crate::run(&wasi, "npm", &["test"])?;
    }
    Ok(())
}

/// Why the npm package at `dir` cannot be tested here, or `None` when it
/// can. The suites run their `.ts` under Node's type stripping and use
/// `using` declarations, which Node has from 24.
fn npm_missing(dir: &Path) -> Option<&'static str> {
    if !have("npm") {
        return Some("npm not found — skipping (install Node.js to run them).");
    }
    let major = Command::new("node")
        .args(["-p", "process.versions.node.split('.')[0]"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if major < 24 {
        return Some("Node <24 — skipping (the suite uses `using` declarations, which Node's type stripping cannot downlevel).");
    }
    // A package with no dependencies (fig-wasi) never has a node_modules.
    let declares = std::fs::read_to_string(dir.join("package.json")).is_ok_and(|p| p.contains("ependencies\""));
    if declares && !dir.join("node_modules").is_dir() {
        return Some("node_modules missing — skipping (run `npm ci` in the package first).");
    }
    None
}
