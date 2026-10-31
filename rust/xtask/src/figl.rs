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
//! first, and found where that build says it wrote it, so a
//! `CARGO_TARGET_DIR` or a cargo config's `build.target-dir` is followed.

use std::path::{Path, PathBuf};
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
    let fig = build_fig(&root)?;
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

/// Build the CLI and return the path of the binary that build wrote.
fn build_fig(root: &Path) -> crate::Result<PathBuf> {
    let args = ["build", "--quiet", "--message-format=json-render-diagnostics", "-p", "fig-cli", "--bin", "fig"];
    eprintln!("· cargo {}", args.join(" "));
    // stdout is cargo's messages, read below; stderr is passed through.
    let out = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("`cargo {}` failed ({})", args.join(" "), out.status));
    }
    executable(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "cargo built fig but did not say where".into())
}

/// The `executable` of the last compiler-artifact message in cargo's
/// `--message-format=json` output that has one. xtask has no JSON parser, and
/// the field is a string cargo writes on one line, so it is read in place.
fn executable(messages: &str) -> Option<PathBuf> {
    const KEY: &str = "\"executable\":\"";
    messages
        .lines()
        .rev()
        .filter(|line| line.contains("\"reason\":\"compiler-artifact\""))
        .find_map(|line| json_string(&line[line.find(KEY)? + KEY.len()..]))
        .map(PathBuf::from)
}

/// The JSON string whose body starts `text`, up to its closing quote,
/// unescaped. `None` for an escape a path would not carry.
fn json_string(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                c @ ('"' | '\\' | '/') => out.push(c),
                _ => return None,
            },
            c => out.push(c),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_is_where_cargo_wrote_the_binary() {
        let messages = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"fig_core"},"filenames":["/t/debug/libfig_core.rlib"],"executable":null,"fresh":true}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"fig"},"filenames":["/tmp/elsewhere/debug/fig"],"executable":"/tmp/elsewhere/debug/fig","fresh":false}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\n",
        );
        assert_eq!(executable(messages), Some(PathBuf::from("/tmp/elsewhere/debug/fig")));
    }

    #[test]
    fn executable_unescapes_a_windows_path() {
        let messages = r#"{"reason":"compiler-artifact","executable":"C:\\t\\debug\\fig.exe","fresh":true}"#;
        assert_eq!(executable(messages), Some(PathBuf::from(r"C:\t\debug\fig.exe")));
    }

    #[test]
    fn executable_is_none_without_a_binary() {
        assert_eq!(executable(r#"{"reason":"compiler-artifact","executable":null}"#), None);
        assert_eq!(executable(r#"{"reason":"build-finished","success":true}"#), None);
        assert_eq!(executable(""), None);
    }
}
