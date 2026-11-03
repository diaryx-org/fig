//! `npm-test <typescript|wasi>`: an npm package's suite, as the `typescript`
//! and `wasi` jobs in `.config/ship.figl` run it — `npm run build`, then
//! `npm test`, and for the TypeScript binding every language's edit fixtures
//! through it.
//!
//! Where the toolchain is missing — no npm, Node older than 24, a package
//! whose `node_modules` was never installed, or for fig-wasi no
//! `wasm32-wasip1` target — it skips with a note and succeeds, so a
//! contributor with Rust alone still gets a useful `ship ci`. The `typescript`
//! and `wasi` jobs in `.github/workflows/ci.yml` install all of it and run the
//! suites themselves, so there they run for real.

use std::path::Path;
use std::process::Command;

use crate::{have, root};

/// What fig-wasi's `npm run build` compiles fig's CLI for.
const WASI_TARGET: &str = "wasm32-wasip1";

pub fn run(args: &[String]) -> crate::Result {
    let (package, extra) = match args {
        [p] if p == "typescript" => ("typescript", true),
        [p] if p == "wasi" => ("wasi", false),
        _ => return Err("takes one package: `typescript` or `wasi`".into()),
    };
    let dir = root().join("bindings").join(package);
    let wasi_target_missing = || {
        (package == "wasi" && !target_installed(WASI_TARGET))
            .then_some("wasm32-wasip1 target missing — skipping (run `rustup target add wasm32-wasip1`).")
    };
    if let Some(why) = npm_missing(&dir).or_else(wasi_target_missing) {
        eprintln!("{package} tests: {why}");
        return Ok(());
    }
    crate::run(&dir, "npm", &["run", "build"])?;
    crate::run(&dir, "npm", &["test"])?;
    if extra {
        crate::run(&dir, "node", &["--conditions=fig-source", "../../tools/edit-fixtures.mjs"])?;
    }
    Ok(())
}

/// Whether the active toolchain has a `std` for `target`. rustc prints the
/// libdir of any target it knows, installed or not, so the answer is whether
/// the directory is there. This asks rustc rather than rustup, which a
/// toolchain from elsewhere (Nix, a distribution) does not have.
fn target_installed(target: &str) -> bool {
    Command::new("rustc")
        .args(["--print", "target-libdir", "--target", target])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|libdir| Path::new(libdir.trim()).is_dir())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> String {
        let out = Command::new("rustc").args(["--print", "host-tuple"]).output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn the_host_target_is_installed() {
        assert!(target_installed(&host()));
    }

    /// rustc knows this target, and rustup ships no `std` for it, so its
    /// libdir is a path that is printed and never there.
    #[test]
    fn a_target_rustc_knows_and_the_toolchain_lacks_is_not_installed() {
        assert!(!target_installed("msp430-none-elf"));
    }

    #[test]
    fn a_target_rustc_does_not_know_is_not_installed() {
        assert!(!target_installed("no-such-target"));
    }
}
