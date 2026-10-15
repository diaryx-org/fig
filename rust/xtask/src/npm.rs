//! `npm-test <typescript|wasi>`: an npm package's suite, as the `typescript`
//! and `wasi` jobs in `.config/ship.figl` run it — `npm run build`, then
//! `npm test`, and for the TypeScript binding every language's edit fixtures
//! through it.
//!
//! Where the toolchain is missing — no npm, Node older than 24, or a package
//! whose `node_modules` was never installed — it skips with a note and
//! succeeds, so a contributor with Rust alone still gets a useful `ship ci`.
//! The `typescript` and `wasi` jobs in `.github/workflows/ci.yml` install
//! Node 24 and run `npm ci` first, so there it runs for real.

use std::path::Path;
use std::process::Command;

use crate::{have, root};

pub fn run(args: &[String]) -> crate::Result {
    let (package, extra) = match args {
        [p] if p == "typescript" => ("typescript", true),
        [p] if p == "wasi" => ("wasi", false),
        _ => return Err("takes one package: `typescript` or `wasi`".into()),
    };
    let dir = root().join("bindings").join(package);
    if let Some(why) = npm_missing(&dir) {
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
