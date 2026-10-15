//! The command line's rules, end to end through the built `fig`: every usage
//! error exits 2 with nothing on stdout and no file touched, and the edge
//! cases `fig --help` promises — `--` ends the flags, `-` is stdin, a
//! negative number is a value, an option's value is the next argument
//! whatever it looks like, opposites resolve to the last one given, and any
//! other action goes to `fig-<action>` with its arguments untouched.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const FIG: &str = env!("CARGO_BIN_EXE_fig");

/// A scratch directory of its own per case, holding `f.yaml` (`a: 1`), a
/// copy named `-x.yaml`, and `p.yaml` (`b: 2`).
fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("fig-usage-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("f.yaml"), "a: 1\n").unwrap();
    fs::write(dir.join("-x.yaml"), "a: 1\n").unwrap();
    fs::write(dir.join("p.yaml"), "b: 2\n").unwrap();
    dir
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn fig_in(dir: &Path, args: &[&str], stdin: &str, path_env: Option<&str>) -> Run {
    let mut cmd = Command::new(FIG);
    cmd.args(args).current_dir(dir).env("NO_COLOR", "1");
    if let Some(p) = path_env {
        cmd.env("PATH", p);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Usage errors: each exits 2, prints nothing on stdout, and leaves the
/// files as they were.
#[test]
fn usage_errors_exit_2_and_touch_nothing() {
    let cases: &[(&str, &[&str])] = &[
        // An unknown flag, in every action — never a file name.
        ("unknown flag: get", &["get", "--bogus", "f.yaml"]),
        ("unknown flag: set", &["set", "--bogus", "f.yaml", "a", "1"]),
        ("unknown flag: replace", &["replace", "f.yaml", "a", "1", "--bogus"]),
        ("unknown flag: rename --string", &["rename", "--string", "f.yaml", "a", "c"]),
        ("unknown short flag: insert", &["insert", "-x", "f.yaml", "c", "1"]),
        ("unknown flag: delete", &["delete", "--bogus", "f.yaml", "a"]),
        ("unknown flag: comment", &["comment", "--bogus", "f.yaml", "a", "hi"]),
        ("unknown flag: check", &["check", "--bogus", "f.yaml"]),
        ("unknown flag: fmt", &["fmt", "--bogus", "f.yaml"]),
        ("unknown flag: convert", &["convert", "--bogus", "-o", "json", "f.yaml"]),
        ("unknown flag: patch", &["patch", "--bogus", "f.yaml", "p.yaml"]),
        ("unknown flag: lang", &["lang", "--bogus"]),
        ("unknown flag: help", &["help", "--bogus"]),
        ("unknown flag: --help", &["--help", "--bogus"]),
        ("unknown flag: version", &["version", "--bogus"]),
        ("unknown flag before the action", &["-q", "get", "f.yaml"]),
        ("a dropped flag: --key", &["replace", "--key", "f.yaml", "a", "c"]),
        ("a dropped flag: --embed", &["get", "--embed", "frontmatter", "f.yaml"]),
        // -inf is a flag: fig has no bare -inf, so it goes after `--`.
        ("-inf reads as a flag", &["set", "f.yaml", "a", "-inf"]),
        // One positional more than the action takes.
        ("surplus: get", &["get", "f.yaml", "a", "b"]),
        ("surplus: set", &["set", "f.yaml", "a", "1", "2"]),
        ("surplus: replace", &["replace", "f.yaml", "a", "1", "2"]),
        ("surplus: rename", &["rename", "f.yaml", "a", "c", "d"]),
        ("surplus: insert", &["insert", "f.yaml", "c", "1", "2"]),
        ("surplus: delete", &["delete", "f.yaml", "a", "b"]),
        ("surplus: comment", &["comment", "f.yaml", "a", "hi", "there"]),
        ("surplus: comment --get", &["comment", "--get", "f.yaml", "a", "b"]),
        ("surplus: fmt", &["fmt", "f.yaml", "a"]),
        ("surplus: convert", &["convert", "-o", "json", "f.yaml", "a"]),
        ("surplus: patch", &["patch", "f.yaml", "p.yaml", "a"]),
        ("surplus: lang list", &["lang", "list", "x"]),
        ("surplus: lang table", &["lang", "table", "f.yaml", "p.yaml"]),
        ("surplus: help", &["help", "set"]),
        ("surplus: version", &["version", "extra"]),
        ("surplus: --version", &["--version", "extra"]),
        // A missing argument.
        ("missing: get", &["get"]),
        ("missing: set", &["set", "f.yaml", "a"]),
        ("missing: replace", &["replace", "f.yaml", "a"]),
        ("missing: rename", &["rename", "f.yaml", "a"]),
        ("missing: insert", &["insert", "f.yaml", "c"]),
        ("missing: delete", &["delete", "f.yaml"]),
        ("missing: comment text", &["comment", "f.yaml", "a"]),
        ("missing: check", &["check"]),
        ("missing: fmt", &["fmt"]),
        ("missing: convert --output", &["convert", "f.yaml"]),
        ("missing: patch file", &["patch", "f.yaml"]),
        ("missing: lang check name", &["lang", "check"]),
        ("missing: lang table file", &["lang", "table"]),
        ("missing: an option's value", &["get", "f.yaml", "--gron-sep"]),
        ("missing: --lang's value", &["get", "f.yaml", "--lang"]),
        // Values that do not parse.
        ("a path that does not parse", &["get", "f.yaml", "a["]),
        ("a --at path that does not parse", &["patch", "--at", "x[", "f.yaml", "p.yaml"]),
        ("an unknown format", &["get", "-i", "bogus", "f.yaml"]),
        ("an unknown --lang", &["get", "f.yaml", "--lang", "nosuch"]),
        ("an --indent that is not a number", &["get", "f.yaml", "--indent", "x"]),
        ("a --seq strategy that is not one", &["patch", "--seq", "x", "f.yaml", "p.yaml"]),
        ("an unknown lang verb", &["lang", "bogus"]),
        // Flags that cannot go together.
        ("--string with --raw", &["set", "--string", "--raw", "f.yaml", "a", "x"]),
        ("fmt --dry-run with --diff", &["fmt", "--dry-run", "--diff", "f.yaml"]),
        ("comment --delete with text", &["comment", "--delete", "f.yaml", "a", "x"]),
        ("patch --strip-comments with --comments ours", &["patch", "--strip-comments", "--comments", "ours", "f.yaml", "p.yaml"]),
        ("patch --delete of the root", &["patch", "--delete", "", "f.yaml", "p.yaml"]),
        ("patch with both files stdin", &["patch", "-", "-"]),
        ("convert --dry-run", &["convert", "--dry-run", "-o", "json", "f.yaml"]),
        // An action fig has no verb for, and no `fig-<action>` provides.
        ("no such action", &["nosuchaction"]),
        ("no such action, not a program name", &["config.toml"]),
        ("edit is gone", &["edit", "f.yaml", "a", "2"]),
    ];
    let mut failures = Vec::new();
    for (label, args) in cases {
        let dir = scratch();
        let r = fig_in(&dir, args, "", Some(""));
        if r.code != 2 {
            failures.push(format!("{label}: `fig {}` exited {}, want 2\n{}", args.join(" "), r.code, r.stderr));
        } else if !r.stdout.is_empty() {
            failures.push(format!("{label}: `fig {}` printed to stdout: {}", args.join(" "), r.stdout));
        } else if fs::read_to_string(dir.join("f.yaml")).unwrap() != "a: 1\n" {
            failures.push(format!("{label}: `fig {}` changed f.yaml", args.join(" ")));
        } else if dir.join("--bogus").exists() {
            failures.push(format!("{label}: `fig {}` created a file named --bogus", args.join(" ")));
        }
        let _ = fs::remove_dir_all(&dir);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The edge cases that are not errors: (args, stdin, exit status, stdout,
/// then `a` in f.yaml, when the case edits it).
#[test]
fn edge_cases_that_succeed() {
    type Case<'a> = (&'a str, &'a [&'a str], &'a str, i32, &'a str, Option<&'a str>);
    let cases: &[Case] = &[
        ("-- ends the flags", &["get", "--", "-x.yaml", "a"], "", 0, "1\n", None),
        ("a value after --", &["set", "f.yaml", "a", "--", "--dash"], "", 0, "", Some("--dash")),
        ("-- -inf is the string", &["set", "f.yaml", "a", "--", "-inf"], "", 0, "", Some("-inf")),
        ("a negative integer", &["set", "f.yaml", "a", "-5"], "", 0, "", Some("-5")),
        ("a negative float", &["set", "f.yaml", "a", "-2.5"], "", 0, "", Some("-2.5")),
        ("negative --seq items",&["set", "--seq", "f.yaml", "l", "-1", "-2"], "", 0, "", None),
        ("- is stdin", &["get", "-", "-i", "yaml", "a"], "a: 7\n", 0, "7\n", None),
        ("check - is stdin", &["check", "-q", "-i", "yaml", "-"], "a: 7\n", 0, "", None),
        ("an option's value may look like a flag", &["get", "f.yaml", "-o", "gron", "--gron-sep", "--"], "", 0, "json--{};\njson.a--1;\n", None),
        ("an option's value may be empty", &["get", "f.yaml", "-o", "gron", "--gron-term", ""], "", 0, "json = {}\njson.a = 1\n", None),
        ("a flag given twice", &["get", "-q", "--quiet", "f.yaml", "a"], "", 0, "1\n", None),
        ("--no-warnings is --quiet", &["get", "--no-warnings", "f.yaml", "a"], "", 0, "1\n", None),
        ("the last of --compact/--pretty wins", &["get", "f.yaml", "-o", "json", "--pretty", "--compact"], "", 0, "{\"a\":1}\n", None),
        ("flags after the positionals", &["replace", "f.yaml", "a", "x", "--string"], "", 0, "", Some("x")),
        ("--lang before the action", &["--lang", "yaml", "get", "f.yaml", "a"], "", 0, "1\n", None),
        ("an alias", &["g", "f.yaml", "a"], "", 0, "1\n", None),
        ("comment --get with no text", &["comment", "--get", "f.yaml", "a"], "", 1, "", None),
        ("help --oneline is one line on stdout", &["help", "--oneline"], "", 0, "fig: read and edit JSON, YAML, TOML, INI, dotenv, plist and other config files by path, keeping comments and formatting. More: `fig help`.\n", None),
        ("fig alone is its help, on stderr", &[], "", 0, "", None),
        ("help is on stderr", &["help"], "", 0, "", None),
        ("-h is help", &["-h"], "", 0, "", None),
        ("help --help", &["help", "--help"], "", 0, "", None),
    ];
    let mut failures = Vec::new();
    for (label, args, stdin, code, stdout, a) in cases {
        let dir = scratch();
        let r = fig_in(&dir, args, stdin, None);
        // `help --help`'s page goes to stdout; its contents are clap's.
        let stdout_ok = if *args == ["help", "--help"] { !r.stdout.is_empty() } else { r.stdout == *stdout };
        if r.code != *code || !stdout_ok {
            failures.push(format!("{label}: `fig {}` exited {} with {:?}, want {code} with {stdout:?}\n{}", args.join(" "), r.code, r.stdout, r.stderr));
        }
        if let Some(want) = a {
            let got = fig_in(&dir, &["get", "f.yaml", "a"], "", None).stdout;
            if got.trim_end_matches('\n') != *want {
                failures.push(format!("{label}: a is {got:?}, want {want:?}"));
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every action's `--help` exits 0 and prints its page on stdout; the
/// general help lists every action.
#[test]
fn help_pages() {
    let dir = scratch();
    for action in ["help", "version", "replace", "rename", "set", "insert", "delete", "get", "comment", "check", "fmt", "convert", "patch", "lang"] {
        for flag in ["--help", "-h"] {
            let r = fig_in(&dir, &[action, flag], "", None);
            assert_eq!(r.code, 0, "fig {action} {flag}: {}", r.stderr);
            assert!(r.stdout.contains("Usage:"), "fig {action} {flag} printed no usage: {}", r.stdout);
        }
    }
    let general = fig_in(&dir, &["help"], "", None).stderr;
    for action in ["help", "version", "replace", "rename", "set", "insert", "delete", "get", "comment", "check", "fmt", "convert", "patch", "lang"] {
        assert!(general.lines().any(|l| l.trim_start().starts_with(action)), "fig help does not list {action}:\n{general}");
    }
    assert!(general.contains("Exit status"), "{general}");
    let _ = fs::remove_dir_all(&dir);
}

/// Any other action runs `fig-<action>` from PATH with every argument after
/// it untouched — fig's own flags and `--` included — and exits with its
/// status.
#[cfg(unix)]
#[test]
fn external_actions_get_their_arguments_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let tool = bin.join("fig-echoargs");
    fs::write(&tool, "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\"; done\necho\nexit 3\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let r = fig_in(&dir, &["echoargs", "--input", "x", "-q", "--", "-5", "--help", "--lang", "y"], "", Some(&path));
    assert_eq!(r.code, 3, "{}", r.stderr);
    assert_eq!(r.stdout, "[--input][x][-q][--][-5][--help][--lang][y]\n");
    let _ = fs::remove_dir_all(&dir);
}
