//! fig's own tasks, run as `cargo xtask <task>` from anywhere in the
//! repository (`.cargo/config.toml` names the alias): the release guards,
//! and `ci`, the one pre-release gate that runs them all.

mod abi;
mod ci;
mod figl;
mod semver;
mod vendor;
mod version;

use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
cargo xtask <task>

  ci              the pre-release gate: every task below that checks, the
                  workspace's tests, the CLI checks and the TypeScript and
                  WASI suites (each skipped with a note without its toolchain)
  version-sync    copy the workspace's version into every file that carries it
  version-check   fail if a file that carries fig's version disagrees
  vendor          copy the license texts and READMEs the published crates need
  vendor-check    fail if a path `vendor` copies from or into is missing
  abi-check       check the C ABI: fig.h against libfig's exports, the C and
                  C++ header probes, a C-hosted language, languages' own libraries
  semver-check    diff the C ABI against the last release tag and check the bump
  sync-figl       regenerate the workflow files from their .figl sources
  check-figl      fail if a workflow file is stale against its .figl source
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(task) = args.first() else {
        eprint!("{HELP}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];
    let result = match task.as_str() {
        "ci" => ci::run(rest),
        "version-sync" => version::run(false),
        "version-check" => version::run(true),
        "vendor" => vendor::run(false),
        "vendor-check" => vendor::run(true),
        "abi-check" => abi::run(rest),
        "semver-check" => semver::run(rest),
        "sync-figl" => figl::run(false),
        "check-figl" => figl::run(true),
        "help" | "-h" | "--help" => {
            print!("{HELP}");
            Ok(())
        }
        other => Err(format!("no task named `{other}`; `cargo xtask help` lists them")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask {task}: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The repository's root: where the workspace's `Cargo.toml` is.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository root exists")
}

/// The result every task returns: `Err` is the message it fails with.
pub type Result<T = ()> = std::result::Result<T, String>;

/// Run `program args` in `dir`, its output passed through; fails when it
/// cannot start or exits nonzero.
pub fn run(dir: &std::path::Path, program: &str, args: &[&str]) -> Result {
    eprintln!("· {program} {}", args.join(" "));
    let status = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("`{program} {}` failed ({status})", args.join(" "))) }
}

/// Whether `program` is on PATH.
pub fn have(program: &str) -> bool {
    std::process::Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
}
