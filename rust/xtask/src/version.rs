//! `version-sync` / `version-check`: copy fig's one version out of the root
//! `Cargo.toml` into every other file that carries it, or (`version-check`)
//! fail if any of them disagrees.
//!
//! fig ships every artifact — the Rust crates, the CLI, the C library,
//! `@diaryx/fig`, `@diaryx/fig-wasi` and each language's npm package — under
//! one version and one `v*` tag (see docs/VERSIONING.md). The version is
//! decided in the root `Cargo.toml`'s `[workspace.package] version`, which
//! every crate of the workspace inherits; `release` moves it there, then
//! runs `cargo xtask version-sync` for the files Cargo's inheritance does not
//! reach. `cargo xtask version-check` is the read-only half, and
//! `cargo xtask ci` runs it.
//!
//! The files, and the fields in each:
//!   * `bindings/c/include/fig.h`: `FIG_VERSION_MAJOR` / `_MINOR` / `_PATCH`
//!   * `README.md`: the frontmatter's `version`
//!   * `Cargo.toml`: every internal `{ path, version }` pin under
//!     `[workspace.dependencies]`, which a published crate needs to name the
//!     version of a sibling it depends on (`[workspace.package] version`
//!     itself is the source, so it always agrees)
//!   * `bindings/typescript/package.json` and `bindings/wasi/package.json`
//!   * both `package-lock.json` files: the root `version` and `packages[""]`,
//!     and in the TypeScript binding's, each language package it links for its
//!     tests (`../../languages/<name>`): its `version` and its `@diaryx/fig` pin
//!   * `bindings/nushell/Cargo.toml`: the `fig = { path, version }` pin; the
//!     plugin is its own workspace on its own version, and depends on fig by
//!     path
//!   * `bindings/nushell/Cargo.lock`: the locked version of each fig crate it
//!     reaches by path, so `--locked` still builds
//!   * each `languages/<name>/package.json`: its `version` and its exact pin
//!     on `@diaryx/fig`, whose module its own must match
//!
//! The root `Cargo.lock` is not here: Cargo owns it, and after a sync that
//! moved a pin `cargo update --workspace` rewrites it (the task says so).
//!
//! `FIG_ABI_VERSION` / `abi_version` is deliberately not here: it is the C ABI's
//! own contract integer, bumped only on a breaking ABI change, and it has no tag.
//!
//! Scanners rather than parsers: the xtask takes no dependencies (no TOML or
//! JSON crate), and each field is one line a substring search finds exactly.
//! More to the point, a parse-and-reserialise would reformat the file; a value
//! spliced in place leaves every byte around it as it was.

use std::ops::Range;
use std::path::Path;

/// Where the version is decided.
const SOURCE: &str = "Cargo.toml";

/// Every file the version is copied into, in the order they are reported,
/// before the language packages (`languages/<name>/package.json`, appended by
/// [`targets`]).
const FIXED_TARGETS: &[&str] = &[
    "bindings/c/include/fig.h",
    "README.md",
    "Cargo.toml",
    "bindings/typescript/package.json",
    "bindings/typescript/package-lock.json",
    "bindings/wasi/package.json",
    "bindings/wasi/package-lock.json",
    "bindings/nushell/Cargo.toml",
    "bindings/nushell/Cargo.lock",
];

/// One field: where its value sits in the file's text, what it should read,
/// and what to call it in a report.
#[derive(Debug)]
struct Edit {
    range: Range<usize>,
    value: String,
    what: &'static str,
}

pub fn run(check: bool) -> crate::Result {
    sync(&crate::root(), check)
}

/// The task, against the repository at `root`.
fn sync(root: &Path, check: bool) -> crate::Result {
    let source = read(root, SOURCE)?;
    let range = cargo_workspace_version_range(&source)
        .ok_or_else(|| format!("no `[workspace.package] version` in {SOURCE}"))?;
    let version = &source[range];
    let parts = semver(version)
        .ok_or_else(|| format!("`[workspace.package] version` in {SOURCE} is `{version}`, which is not a SemVer version"))?;

    let mut failed = false;
    let mut cargo_moved = false;
    for rel in targets(root)? {
        let text = read(root, &rel)?;
        let Some(edits) = plan(&rel, &text, parts, version) else {
            eprintln!("version-sync: FAIL    {rel}: a version field is missing — the scanner could not find it");
            failed = true;
            continue;
        };

        let mut stale = 0;
        for e in &edits {
            let have = &text[e.range.clone()];
            if have != e.value {
                stale += 1;
                if check {
                    eprintln!("version-sync: STALE   {rel}: {} is {have}, want {}", e.what, e.value);
                }
            }
        }

        if stale == 0 {
            eprintln!("version-sync: ok      {rel}");
            continue;
        }
        if check {
            failed = true;
            continue;
        }
        let path = root.join(&rel);
        std::fs::write(&path, splice(&text, &edits)).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        eprintln!("version-sync: updated {rel} ({stale} field{} -> {version})", if stale == 1 { "" } else { "s" });
        if rel == "Cargo.toml" {
            cargo_moved = true;
        }
    }

    if failed {
        return Err(format!("{SOURCE} says {version}; run `cargo xtask version-sync` to copy it everywhere"));
    }
    if cargo_moved {
        eprintln!("version-sync: now refresh the lockfile: cargo update --workspace");
    }
    Ok(())
}

/// [`FIXED_TARGETS`], then `languages/<name>/package.json` for every language
/// fig ships: each directory under `languages/` with a `Cargo.toml`, the same
/// set the workspace's `languages/*` member glob takes, in name order.
fn targets(root: &Path) -> crate::Result<Vec<String>> {
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
    let mut out: Vec<String> = FIXED_TARGETS.iter().map(|s| s.to_string()).collect();
    out.extend(names.iter().map(|n| format!("languages/{n}/package.json")));
    Ok(out)
}

fn read(root: &Path, rel: &str) -> crate::Result<String> {
    let path = root.join(rel);
    std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// `major.minor.patch` of a SemVer version, which may carry a `-pre` and a
/// `+build` suffix; `None` if `v` is not one.
fn semver(v: &str) -> Option<(u64, u64, u64)> {
    let (core, build) = v.split_once('+').map_or((v, None), |(c, b)| (c, Some(b)));
    let (core, pre) = core.split_once('-').map_or((core, None), |(c, p)| (c, Some(p)));
    let ident_ok = |s: &str| {
        !s.is_empty() && s.split('.').all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    };
    if pre.is_some_and(|p| !ident_ok(p)) || build.is_some_and(|b| !ident_ok(b)) {
        return None;
    }
    let num = |s: &str| {
        let leading_zero = s.len() > 1 && s.starts_with('0');
        if s.is_empty() || leading_zero || !s.bytes().all(|b| b.is_ascii_digit()) { None } else { s.parse().ok() }
    };
    let mut it = core.split('.');
    let parts = (num(it.next()?)?, num(it.next()?)?, num(it.next()?)?);
    if it.next().is_some() { None } else { Some(parts) }
}

/// Every version field in the file at `rel`, with the value it should hold;
/// `None` if a field the file must carry is not there.
fn plan(rel: &str, text: &str, (major, minor, patch): (u64, u64, u64), version: &str) -> Option<Vec<Edit>> {
    let field = |range, what| Edit { range, value: version.to_string(), what };
    let mut out = Vec::new();
    if rel.ends_with("fig.h") {
        for (macro_, n) in [("FIG_VERSION_MAJOR", major), ("FIG_VERSION_MINOR", minor), ("FIG_VERSION_PATCH", patch)] {
            out.push(Edit { range: c_macro_int_range(text, macro_)?, value: n.to_string(), what: macro_ });
        }
    } else if rel.ends_with("README.md") {
        out.push(field(frontmatter_version_range(text)?, "frontmatter version"));
    } else if rel == "bindings/nushell/Cargo.toml" {
        out.push(field(cargo_path_pin_range(text, "fig")?, "the fig dependency pin"));
    } else if rel.ends_with("Cargo.lock") {
        let locked = cargo_lock_fig_ranges(text);
        if locked.is_empty() {
            return None;
        }
        out.extend(locked.into_iter().map(|r| field(r, "a fig crate's locked version")));
    } else if rel.ends_with("Cargo.toml") {
        // The source of the version, so it agrees by construction; it is
        // located anyway, so a manifest that lost it fails here too.
        cargo_workspace_version_range(text)?;
        let pins = cargo_internal_pin_ranges(text);
        if pins.is_empty() {
            return None;
        }
        out.extend(pins.into_iter().map(|r| field(r, "an internal dependency pin")));
    } else if rel.ends_with("package-lock.json") {
        let [own, inner] = package_lock_version_ranges(text)?;
        out.push(field(own, "version"));
        out.push(field(inner, "packages[\"\"].version"));
        out.extend(package_lock_linked_ranges(text).into_iter().map(|r| field(r, "a linked language package's version or pin")));
    } else if rel.ends_with("package.json") {
        out.push(field(json_version_range(text, 0)?, "version"));
        if rel.starts_with("languages/") {
            out.push(field(json_pin_range(text, "@diaryx/fig")?, "the @diaryx/fig pin"));
        }
    } else {
        unreachable!("no scanner for {rel}");
    }
    Some(out)
}

/// `text` with every edit's range replaced by its value. The ranges come from
/// distinct fields and never overlap; they are applied in file order.
fn splice(text: &str, edits: &[Edit]) -> String {
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| e.range.start);
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for e in sorted {
        out.push_str(&text[pos..e.range.start]);
        out.push_str(&e.value);
        pos = e.range.end;
    }
    out.push_str(&text[pos..]);
    out
}

// --- Locators. Each returns the byte range of a field's VALUE (between the
// quotes, where it has them), so a splice leaves every byte around it alone.
// Every search is for ASCII, so every index lands on a char boundary.

/// The end of the line that starts at `i`: the index of the next `\n` at or
/// after `i`, or `text.len()` on the last line.
fn line_end(text: &str, i: usize) -> usize {
    text[i..].find('\n').map_or(text.len(), |n| i + n)
}

/// The contents of the next double-quoted string at or after `idx`.
fn quoted_range_after(text: &str, idx: usize) -> Option<Range<usize>> {
    let open = idx + text[idx..].find('"')?;
    let close = open + 1 + text[open + 1..].find('"')?;
    Some(open + 1..close)
}

/// The bare value of the first `version = X.Y.Z` line in `text[from..to]` —
/// the `fig` frontmatter block of a markdown file. The key has to be exactly
/// `version`, so `minimum_version = …` is never taken for it; the value runs
/// to the first space, `#` comment, or end of line.
fn figl_version_range(text: &str, from: usize, to: usize) -> Option<Range<usize>> {
    let text = &text[..to];
    let mut i = from;
    while i < to {
        let nl = line_end(text, i);
        let rest = text[i..nl].trim_start_matches([' ', '\t']);
        if let Some(after) = rest.strip_prefix("version") {
            let after = after.trim_start_matches([' ', '\t']);
            if after.starts_with('=') {
                let mut s = nl - after.len() + 1;
                let b = text.as_bytes();
                while s < nl && (b[s] == b' ' || b[s] == b'\t') {
                    s += 1;
                }
                let mut e = s;
                while e < nl && !matches!(b[e], b' ' | b'\t' | b'#' | b'\r') {
                    e += 1;
                }
                if e > s {
                    return Some(s..e);
                }
            }
        }
        i = nl + 1;
    }
    None
}

/// The `version` inside a markdown file's leading ```` ```fig ```` frontmatter
/// block — and only there, so a `version = …` in a code sample further down the
/// README is never mistaken for it.
fn frontmatter_version_range(text: &str) -> Option<Range<usize>> {
    const FENCE: &str = "```fig\n";
    if !text.starts_with(FENCE) {
        return None;
    }
    let close = FENCE.len() + text[FENCE.len()..].find("\n```")?;
    figl_version_range(text, FENCE.len(), close + 1)
}

/// The integer of a `#define <macro> <int>` line. Anchors on the first
/// occurrence of `macro` followed by digits — the `#define` precedes every use
/// in `FIG_VERSION_NUM`, and those uses have no digits after the name.
fn c_macro_int_range(text: &str, macro_: &str) -> Option<Range<usize>> {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(n) = text[from..].find(macro_) {
        let at = from + n;
        let mut s = at + macro_.len();
        while s < b.len() && (b[s] == b' ' || b[s] == b'\t') {
            s += 1;
        }
        let mut e = s;
        while e < b.len() && b[e].is_ascii_digit() {
            e += 1;
        }
        if e > s {
            return Some(s..e);
        }
        from = at + macro_.len();
    }
    None
}

/// The first `version = "…"` under `[workspace.package]`, before the next
/// section header.
fn cargo_workspace_version_range(text: &str) -> Option<Range<usize>> {
    let sec = text.find("[workspace.package]")?;
    let mut i = sec + text[sec..].find('\n')? + 1;
    while i < text.len() {
        let nl = line_end(text, i);
        let trimmed = text[i..nl].trim_start_matches([' ', '\t']);
        if trimmed.starts_with('[') {
            return None;
        }
        if trimmed.starts_with("version") {
            let eq = i + text[i..nl].find('=')?;
            return quoted_range_after(&text[..nl], eq + 1);
        }
        i = nl + 1;
    }
    None
}

/// The `version = "…"` of every `name = { path = "…", version = "…" }` line
/// under `[workspace.dependencies]` — the crates of this workspace, which all
/// share the one version. External dependencies carry no `path` and are
/// skipped, and so is a path dependency with no `version`: a crate that is
/// never published, which nothing published may depend on.
fn cargo_internal_pin_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let Some(sec) = text.find("[workspace.dependencies]") else { return out };
    let Some(n) = text[sec..].find('\n') else { return out };
    let mut i = sec + n + 1;
    while i < text.len() {
        let nl = line_end(text, i);
        let line = &text[i..nl];
        if line.trim_start_matches([' ', '\t']).starts_with('[') {
            break;
        }
        if line.contains("path = \"")
            && let Some(r) = line_version_value(text, i, nl)
        {
            out.push(r);
        }
        i = nl + 1;
    }
    out
}

/// The quoted value after the first `version` and its `=` on the line
/// `text[i..nl]`.
fn line_version_value(text: &str, i: usize, nl: usize) -> Option<Range<usize>> {
    let line = &text[..nl];
    let ver = i + line[i..].find("version")?;
    let veq = ver + line[ver..].find('=')?;
    quoted_range_after(line, veq + 1)
}

/// The `version = "…"` of the `name = { path = "…", version = "…" }` line: a
/// dependency on one of this repository's crates from outside its workspace.
/// A line naming a crate whose name only starts with `name` does not match.
fn cargo_path_pin_range(text: &str, name: &str) -> Option<Range<usize>> {
    let mut i = 0;
    while i < text.len() {
        let nl = line_end(text, i);
        let line = &text[i..nl];
        if let Some(rest) = line.trim_start_matches([' ', '\t']).strip_prefix(name)
            && rest.trim_start_matches([' ', '\t']).starts_with('=')
            && line.contains("path = \"")
        {
            return line_version_value(text, i, nl);
        }
        i = nl + 1;
    }
    None
}

/// The `version = "…"` of every `[[package]]` in a Cargo.lock that is one of
/// fig's crates (`fig`, `fig-*`) reached by path — a package with no `source`
/// line. A fig crate from a registry carries a `source`, and is left alone.
fn cargo_lock_fig_ranges(text: &str) -> Vec<Range<usize>> {
    const HEADER: &str = "[[package]]";
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(n) = text[at..].find(HEADER) {
        let start = at + n;
        let end = text[start + 1..].find(HEADER).map_or(text.len(), |n| start + 1 + n);
        let block = &text[start..end];
        at = end;
        if block.contains("\nsource = ") {
            continue;
        }
        let Some(name) = block.find("\nname = ").and_then(|i| quoted_range_after(block, i + 1)) else { continue };
        let name = &block[name];
        if name != "fig" && !name.starts_with("fig-") {
            continue;
        }
        if let Some(ver) = block.find("\nversion = ").and_then(|i| quoted_range_after(block, i + 1)) {
            out.push(start + ver.start..start + ver.end);
        }
    }
    out
}

/// `"<name>": "…"` in a package.json's dependencies, the first key spelled
/// exactly so.
fn json_pin_range(text: &str, name: &str) -> Option<Range<usize>> {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(n) = text[from..].find(name) {
        let at = from + n;
        from = at + name.len();
        let end = at + name.len();
        if at == 0 || b[at - 1] != b'"' || end >= b.len() || b[end] != b'"' {
            continue;
        }
        let colon = end + text[end..].find(':')?;
        return quoted_range_after(text, colon + 1);
    }
    None
}

/// The string after the first `"version"` key at or after `from` in a JSON file.
fn json_version_range(text: &str, from: usize) -> Option<Range<usize>> {
    const KEY: &str = "\"version\"";
    let at = from + text[from..].find(KEY)?;
    let colon = at + KEY.len() + text[at + KEY.len()..].find(':')?;
    quoted_range_after(text, colon + 1)
}

/// A package-lock.json's two copies of the package's own version: the root
/// `version`, and the one under `packages[""]` — the first `"version"` after
/// the `""` key opens.
fn package_lock_version_ranges(text: &str) -> Option<[Range<usize>; 2]> {
    let own = json_version_range(text, 0)?;
    let packages = text.find("\"packages\"")?;
    let this = packages + text[packages..].find("\"\":")?;
    let inner = json_version_range(text, this)?;
    Some([own, inner])
}

/// In a package-lock.json, each language package linked from the repository
/// (`"../../languages/<name>": { … }`): its `version`, and its pin on
/// `@diaryx/fig`. None is not an error: only the TypeScript binding links any.
fn package_lock_linked_ranges(text: &str) -> Vec<Range<usize>> {
    const KEY: &str = "\"../../languages/";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(n) = text[from..].find(KEY) {
        let at = from + n;
        from = at + KEY.len();
        let Some(close) = text[from..].find('"').map(|n| from + n) else { break };
        // A key, not a `"resolved"` value: a `:` and a `{` follow it.
        if !text[close + 1..].trim_start_matches(' ').starts_with(": {") {
            continue;
        }
        let end = text[close..].find("\n    }").map_or(text.len(), |n| close + n);
        let entry = &text[at..end];
        let shift = |r: Range<usize>| at + r.start..at + r.end;
        out.extend(json_version_range(entry, 0).map(shift));
        out.extend(json_pin_range(entry, "@diaryx/fig").map(shift));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(rel: &str, text: &str, v: &str) -> String {
        let edits = plan(rel, text, semver(v).unwrap(), v).expect("every field found");
        splice(text, &edits)
    }

    #[test]
    fn semver_takes_versions_and_refuses_the_rest() {
        assert_eq!(semver("6.1.0"), Some((6, 1, 0)));
        assert_eq!(semver("7.0.0-rc.1+build.5"), Some((7, 0, 0)));
        for bad in ["6.1", "6.1.0.0", "06.1.0", "6.x.0", "6.1.0-", "6.1.0+", ""] {
            assert_eq!(semver(bad), None, "{bad}");
        }
    }

    #[test]
    fn fig_h_the_three_macros_and_not_their_uses() {
        let src = "#define FIG_VERSION_MAJOR 3\n\
                   #define FIG_VERSION_MINOR 1\n\
                   #define FIG_VERSION_PATCH 0\n\
                   #define FIG_VERSION_NUM (((uint32_t)FIG_VERSION_MAJOR << 16) | FIG_VERSION_PATCH)\n\
                   #define FIG_ABI_VERSION 2\n";
        let want = "#define FIG_VERSION_MAJOR 10\n\
                    #define FIG_VERSION_MINOR 2\n\
                    #define FIG_VERSION_PATCH 3\n\
                    #define FIG_VERSION_NUM (((uint32_t)FIG_VERSION_MAJOR << 16) | FIG_VERSION_PATCH)\n\
                    #define FIG_ABI_VERSION 2\n";
        assert_eq!(applied("bindings/c/include/fig.h", src, "10.2.3"), want);
    }

    #[test]
    fn readme_the_frontmatter_only() {
        let src = "```fig\ntitle = fig\nversion = 3.1.0\n```\n\n```fig\nversion = 1.0.0\n```\n";
        assert_eq!(
            applied("README.md", src, "5.0.0"),
            "```fig\ntitle = fig\nversion = 5.0.0\n```\n\n```fig\nversion = 1.0.0\n```\n"
        );
    }

    #[test]
    fn cargo_toml_internal_pins_not_external_or_unversioned_deps() {
        let src = "[workspace]\nresolver = \"3\"\n\n[workspace.package]\nversion = \"5.0.0\"\n\n\
                   [workspace.dependencies]\n\
                   fig-kit = { path = \"rust/fig-kit\", version = \"4.1.0\", default-features = false }\n\
                   fig-abi = { path = \"rust/fig-abi\" }\n\
                   syn = { version = \"2\", features = [\"full\"] }\n";
        let want = "[workspace]\nresolver = \"3\"\n\n[workspace.package]\nversion = \"5.0.0\"\n\n\
                    [workspace.dependencies]\n\
                    fig-kit = { path = \"rust/fig-kit\", version = \"5.0.0\", default-features = false }\n\
                    fig-abi = { path = \"rust/fig-abi\" }\n\
                    syn = { version = \"2\", features = [\"full\"] }\n";
        assert_eq!(applied("Cargo.toml", src, "5.0.0"), want);
        assert_eq!(&src[cargo_workspace_version_range(src).unwrap()], "5.0.0");
    }

    #[test]
    fn package_lock_both_copies_of_the_packages_own_version() {
        let src = r#"{
  "name": "@diaryx/fig",
  "version": "3.1.0",
  "packages": {
    "": {
      "name": "@diaryx/fig",
      "version": "3.1.0",
      "devDependencies": { "typescript": "^5" }
    },
    "node_modules/typescript": { "version": "5.9.0" }
  }
}"#;
        let got = applied("bindings/typescript/package-lock.json", src, "5.0.0");
        assert_eq!(got.matches("\"version\": \"5.0.0\"").count(), 2);
        assert!(got.contains("\"version\": \"5.9.0\""));
    }

    #[test]
    fn nushell_cargo_toml_the_fig_pin_not_a_crate_whose_name_starts_with_fig() {
        let src = "[package]\nname = \"nu_plugin_fig\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
                   figment = { path = \"../figment\", version = \"1.0.0\" }\n\
                   fig = { path = \"../rust/fig\", version = \"5.0.0\" }\n";
        let got = applied("bindings/nushell/Cargo.toml", src, "5.1.0");
        assert!(got.contains("fig = { path = \"../rust/fig\", version = \"5.1.0\" }"));
        assert!(got.contains("version = \"0.1.0\""));
        assert!(got.contains("version = \"1.0.0\""));
    }

    #[test]
    fn cargo_lock_figs_path_crates_not_the_root_or_a_registry_package() {
        let src = "[[package]]\nname = \"fig\"\nversion = \"5.0.0\"\ndependencies = [\n \"fig-kit\",\n]\n\n\
                   [[package]]\nname = \"fig-kit\"\nversion = \"5.0.0\"\n\n\
                   [[package]]\nname = \"figment\"\nversion = \"5.0.0\"\n\
                   source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
                   [[package]]\nname = \"nu_plugin_fig\"\nversion = \"0.1.0\"\n";
        let got = applied("bindings/nushell/Cargo.lock", src, "5.1.0");
        assert_eq!(got.matches("version = \"5.1.0\"").count(), 2);
        assert!(got.contains("name = \"figment\"\nversion = \"5.0.0\""));
        assert!(got.contains("version = \"0.1.0\""));
    }

    #[test]
    fn a_language_packages_version_and_its_diaryx_fig_pin() {
        let src = r#"{
  "name": "@diaryx/fig-yaml",
  "version": "6.0.0",
  "homepage": "https://github.com/diaryx-org/fig",
  "peerDependencies": {
    "@diaryx/fig": "6.0.0"
  }
}"#;
        let got = applied("languages/yaml/package.json", src, "6.1.0");
        assert!(got.contains("\"version\": \"6.1.0\""));
        assert!(got.contains("\"@diaryx/fig\": \"6.1.0\""));
    }

    #[test]
    fn the_language_packages_a_package_lock_links() {
        let src = r#"{
  "name": "@diaryx/fig",
  "version": "6.0.0",
  "packages": {
    "": {
      "name": "@diaryx/fig",
      "version": "6.0.0"
    },
    "../../languages/yaml": {
      "name": "@diaryx/fig-yaml",
      "version": "6.0.0",
      "peerDependencies": {
        "@diaryx/fig": "6.0.0"
      }
    },
    "node_modules/@diaryx/fig-yaml": {
      "resolved": "../../languages/yaml",
      "link": true
    }
  }
}"#;
        let got = applied("bindings/typescript/package-lock.json", src, "6.1.0");
        assert_eq!(got.matches("6.0.0").count(), 0);
        assert_eq!(got.matches("6.1.0").count(), 4);
    }

    #[test]
    fn a_missing_field_is_reported_not_skipped() {
        assert!(plan("README.md", "# fig\n", (1, 0, 0), "1.0.0").is_none());
        assert!(plan("languages/yaml/package.json", "{ \"version\": \"1.0.0\" }", (1, 0, 0), "1.0.0").is_none());
    }
}
