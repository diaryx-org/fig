//! `semver-check`: the C ABI diffed against the last release tag, turned into
//! a SemVer verdict. Where `abi-check` proves the header and the library agree
//! at one point in time, this compares the working tree's fig.h against the
//! header as it was at the most recent release tag and classifies the delta:
//!
//!   * a removed function, or a changed signature (return type / parameters)
//!     -> MAJOR (breaking)
//!   * only new functions added -> MINOR (feature)
//!   * no change to the function surface -> PATCH
//!
//! From that verdict and the baseline's version it computes the least next
//! version that covers it, and fails if the workspace's version (the root
//! `Cargo.toml`'s `[workspace.package] version`) is below it: an ABI delta
//! must be bumped for, and with none the version must not regress below the
//! release (equal to it is fine — that is the tree right after a release).
//! 0.x baselines follow SemVer's "anything goes in 0.x" rule (a break is a
//! minor bump, a feature a patch bump).
//!
//! A breaking delta must ALSO move fig.h's `FIG_ABI_VERSION` (the binary
//! contract counter, distinct from the marketing version) past the
//! baseline's, so a break never ships under the same ABI number. A baseline
//! that predates the macro has nothing to compare against, so the rule is
//! waived for it.
//!
//! Baseline discovery: `git describe --tags --abbrev=0` for the most recent
//! release reachable from HEAD. fig releases every artifact under one `vX.Y.Z`
//! tag from 5.0.0 on (docs/VERSIONING.md), matched as `v[5-9].*` or
//! `v[1-9][0-9]*` — which admits v10 and later and leaves out the three bare
//! tags that predate the per-track scheme (`v1.0.0`, `v2.0.0`, `v2.5.1`, all
//! ancestors of HEAD) — falling back to the core's old `core/v*` line. `git
//! show <tag>:<path>` then reads that release's header at its current path,
//! then at its pre-move path (`include/fig.h`). With no git or no matching tag
//! (a source tarball) it prints a note and passes rather than failing a build
//! that has nothing to diff against.
//!
//! Coverage: the function surface (names and whitespace-normalized
//! declarations) and the `typedef struct/enum` surface — field layout (order,
//! type, array size) and enumerator values — under fig's forward-compat policy:
//! a size-gated struct (first field `uint32_t size`), or one marked
//! `// fig-abi: host-written` or `// fig-abi: stride-gated`, may append fields
//! (MINOR); adding an enumerator is MINOR; reordering, retyping or removing a
//! field, or changing an enumerator's value, is MAJOR. C has no name mangling,
//! so this string-level compare of normalized declarations stands in for a
//! symbol-table diff. Not expanded: typedef/macro aliases and `#if`-gated
//! declarations — review those by hand for a major bump.

use std::cmp::Ordering;
use std::path::Path;
use std::process::Command;

const HEADER: &str = "bindings/c/include/fig.h";
/// Where a tag cut before the header moved has it.
const OLD_HEADER: &str = "include/fig.h";

pub fn run(args: &[String]) -> crate::Result {
    if let Some(extra) = args.first() {
        return Err(format!("takes no arguments (got `{extra}`)"));
    }
    let root = crate::root();
    let current = std::fs::read_to_string(root.join(HEADER))
        .map_err(|e| format!("cannot read {HEADER}: {e}"))?;
    let want = workspace_version(&root)?;

    let Some(tag) = git(
        &root,
        &[
            "describe",
            "--tags",
            "--abbrev=0",
            "--match",
            "v[5-9].*",
            "--match",
            "v[1-9][0-9]*",
        ],
    )
    .or_else(|| {
        git(
            &root,
            &["describe", "--tags", "--abbrev=0", "--match", "core/v*"],
        )
    }) else {
        eprintln!(
            "semver-check: no release tag found (no git history, and no v* or core/v* tags) — skipping diff."
        );
        return Ok(());
    };
    let tag = tag.trim().to_string();
    let Some(baseline) = [HEADER, OLD_HEADER]
        .iter()
        .find_map(|path| git(&root, &["show", &format!("{tag}:{path}")]))
    else {
        eprintln!(
            "semver-check: could not read the header at {tag} (tried {HEADER} and {OLD_HEADER}) — skipping diff."
        );
        return Ok(());
    };

    // What shipped is what the header said; the tag's own spelling is the
    // fallback for a header without the macros.
    let base_version = header_version(&baseline)
        .or_else(|| Ver::parse(tag.trim_start_matches("core/").trim_start_matches('v')))
        .ok_or_else(|| format!("cannot tell {tag}'s version from its header or its name"))?;

    // The function surface.
    let base_fns = collect_fns(&baseline);
    let cur_fns = collect_fns(&current);
    let added: Vec<&Fn> = cur_fns
        .iter()
        .filter(|c| !base_fns.iter().any(|b| b.name == c.name))
        .collect();
    let removed: Vec<&Fn> = base_fns
        .iter()
        .filter(|b| !cur_fns.iter().any(|c| c.name == b.name))
        .collect();
    let changed: Vec<(&Fn, &Fn)> = cur_fns
        .iter()
        .filter_map(|c| {
            base_fns
                .iter()
                .find(|b| b.name == c.name && b.sig != c.sig)
                .map(|b| (b, c))
        })
        .collect();
    let mut verdict = if !removed.is_empty() || !changed.is_empty() {
        Verdict::Major
    } else if !added.is_empty() {
        Verdict::Minor
    } else {
        Verdict::Patch
    };

    // The struct/enum surface.
    let base_aggs = collect_aggregates(&baseline);
    let cur_aggs = collect_aggregates(&current);
    let (agg_verdict, agg_lines) = diff_aggregates(&base_aggs, &cur_aggs);
    verdict = verdict.max(agg_verdict);

    // Only a break demands an ABI-version bump; a baseline without the macro
    // waives it; a current header that dropped the macro fails it.
    let base_abi = macro_int(&baseline, "FIG_ABI_VERSION");
    let cur_abi = macro_int(&current, "FIG_ABI_VERSION");
    let abi_ok =
        verdict != Verdict::Major || base_abi.is_none_or(|ba| cur_abi.is_some_and(|ca| ca > ba));
    let abi = |v: Option<u32>| v.map_or("(absent)".to_string(), |v| format!("v{v}"));

    eprintln!("semver-check: C ABI diff (function surface)");
    eprintln!(
        "  baseline {tag} ({} symbols)  ->  working tree ({} symbols)",
        base_fns.len(),
        cur_fns.len()
    );
    for f in &added {
        eprintln!("  + {}", f.sig);
    }
    for f in &removed {
        eprintln!("  - {}", f.sig);
    }
    for (b, c) in &changed {
        eprintln!("  ~ {}\n      was: {}\n      now: {}", b.name, b.sig, c.sig);
    }
    if added.is_empty() && removed.is_empty() && changed.is_empty() {
        eprintln!("  (no change to the exported function surface)");
    }
    eprintln!("semver-check: C ABI diff (struct/enum surface)");
    eprintln!(
        "  baseline ({} aggregates)  ->  working tree ({} aggregates)",
        base_aggs.len(),
        cur_aggs.len()
    );
    for line in &agg_lines {
        eprintln!("  {line}");
    }
    if agg_lines.is_empty() {
        eprintln!("  (no change to struct layout or enum values)");
    }
    eprintln!("semver-check: C ABI contract version (FIG_ABI_VERSION)");
    eprintln!(
        "  baseline {}  ->  working tree {}",
        abi(base_abi),
        abi(cur_abi)
    );

    let suggested = base_version.bump(verdict);
    // A delta demands at least the bump; none demands only no regression.
    let required = if verdict == Verdict::Patch {
        base_version
    } else {
        suggested
    };
    eprintln!(
        "verdict: {}  (suggested next version: {suggested})",
        verdict.name()
    );

    let mut failed = false;
    if want < required {
        eprintln!("Cargo.toml: {want}  ->  FAIL: bump to >= {required} before release");
        failed = true;
    } else {
        eprintln!("Cargo.toml: {want}  ->  OK (covers the ABI delta)");
    }
    if !abi_ok {
        eprintln!(
            "FIG_ABI_VERSION  ->  FAIL: a breaking (MAJOR) ABI change must increment it past the baseline ({})",
            abi(base_abi)
        );
        failed = true;
    }
    if failed {
        eprintln!(
            "note: typedef/macro aliases and #if-gated declarations are not expanded — review those by hand for a major bump."
        );
        return Err("the version does not cover the C ABI delta".into());
    }
    Ok(())
}

/// `git -C <root> <args>`'s stdout on a clean exit, or `None` on any failure
/// (no git, a nonzero exit, an object that is not there).
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Ver {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Ver {
    pub(crate) fn parse(s: &str) -> Option<Ver> {
        let mut it = s.trim().split('.');
        let ver = Ver {
            major: it.next()?.parse().ok()?,
            minor: it.next()?.parse().ok()?,
            patch: it.next()?.parse().ok()?,
        };
        it.next().is_none().then_some(ver)
    }

    fn bump(self, verdict: Verdict) -> Ver {
        let Ver {
            major,
            minor,
            patch,
        } = self;
        match (major, verdict) {
            // 0.x: a break is a minor bump, a feature a patch bump.
            (0, Verdict::Major) => Ver {
                major: 0,
                minor: minor + 1,
                patch: 0,
            },
            (0, _) => Ver {
                major: 0,
                minor,
                patch: patch + 1,
            },
            (_, Verdict::Major) => Ver {
                major: major + 1,
                minor: 0,
                patch: 0,
            },
            (_, Verdict::Minor) => Ver {
                major,
                minor: minor + 1,
                patch: 0,
            },
            (_, Verdict::Patch) => Ver {
                major,
                minor,
                patch: patch + 1,
            },
        }
    }
}

impl std::fmt::Display for Ver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// fig's one version: the root `Cargo.toml`'s `[workspace.package] version`.
/// Read by hand, as the xtask takes no TOML parser: the first `version = "…"`
/// line inside that table.
pub(crate) fn workspace_version(root: &Path) -> crate::Result<Ver> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("cannot read Cargo.toml: {e}"))?;
    let mut in_table = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_table = line == "[workspace.package]";
            continue;
        }
        if !in_table {
            continue;
        }
        if let Some(rest) = line.strip_prefix("version")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let value = value.trim().trim_matches('"');
            return Ver::parse(value).ok_or_else(|| {
                format!("Cargo.toml's workspace version `{value}` is not major.minor.patch")
            });
        }
    }
    Err("Cargo.toml has no [workspace.package] version".into())
}

/// The version a header's `#define FIG_VERSION_*` lines spell, or `None`.
pub(crate) fn header_version(header: &str) -> Option<Ver> {
    Some(Ver {
        major: macro_int(header, "FIG_VERSION_MAJOR")?,
        minor: macro_int(header, "FIG_VERSION_MINOR")?,
        patch: macro_int(header, "FIG_VERSION_PATCH")?,
    })
}

/// The integer `#define <name> <int>` gives in `text`, or `None` if it is
/// absent or not a plain decimal.
pub(crate) fn macro_int(text: &str, name: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        (it.next()? == "#define" && it.next()? == name).then_some(())?;
        Some(it.next().and_then(|v| v.parse().ok()))
    })?
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Verdict {
    Patch,
    Minor,
    Major,
}

impl Verdict {
    fn name(self) -> &'static str {
        match self {
            Verdict::Patch => "patch",
            Verdict::Minor => "minor",
            Verdict::Major => "major",
        }
    }
}

struct Fn {
    name: String,
    /// The whole declaration, whitespace-normalized.
    sig: String,
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Every `fig_*` function declaration, sorted by name. Comments, directives
/// and `extern "C"` braces are removed, the rest split into `;`-ended
/// statements, and a statement whose first `fig_<name>` token is followed
/// by `(` is a declaration whose normalized text is its signature.
fn collect_fns(header: &str) -> Vec<Fn> {
    let text = strip_directives(&strip_comments(header))
        .replace("extern \"C\"", " ")
        .replace(['{', '}'], " ");
    let mut fns: Vec<Fn> = text
        .split(';')
        .filter_map(|stmt| {
            let sig = normalize_ws(stmt);
            let name = function_name(&sig)?.to_string();
            Some(Fn { name, sig })
        })
        .collect();
    fns.sort_by(|a, b| a.name.cmp(&b.name));
    fns
}

/// The first `fig_<name>` in `sig` that is followed by `(` and is not the
/// tail of a longer identifier.
fn function_name(sig: &str) -> Option<&str> {
    let b = sig.as_bytes();
    let mut i = 0;
    while let Some(off) = sig[i..].find("fig_") {
        let pos = i + off;
        if pos > 0 && is_name_char(b[pos - 1]) {
            i = pos + 4;
            continue;
        }
        let mut end = pos + 4;
        while end < b.len() && is_name_char(b[end]) {
            end += 1;
        }
        if b.get(end) == Some(&b'(') {
            return Some(&sig[pos..end]);
        }
        i = end;
    }
    None
}

/// `text` with each `//…` and `/*…*/` comment replaced by one space.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |n| &after[n..]);
            out.push(' ');
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |n| &after[n + 2..]);
            out.push(' ');
        } else {
            let c = rest.chars().next().expect("nonempty");
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// `text` without its preprocessor directives, including the continuation
/// lines of a `\`-ended `#define` (else a multi-line macro body would leak
/// into the next declaration).
fn strip_directives(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut continued = false;
    for line in text.split('\n') {
        if continued || line.trim_start().starts_with('#') {
            continued = line.trim_end().ends_with('\\');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Every run of whitespace collapsed to one space, the ends trimmed.
fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// The struct/enum (aggregate) surface — layout and enumerator values.
//
// A C struct's ABI is positional: a caller compiled against the old header
// reads field N at its old offset, so reordering, retyping, resizing (an array
// dimension too) or removing a field is a break. Appending a field is a break
// in general as well (sizeof changes, so an array of the struct re-strides) —
// except for fig's size-versioned structs, whose first field is `uint32_t
// size`: the library writes only the fields `size` covers, so an older
// caller's smaller layout still works. The marked structs (host-written,
// stride-gated) are read the same prefix-wise. For enums, adding an
// enumerator is additive (callers decode unknown values, per fig.h's
// forward-compat note), but changing one's value reassigns meaning under every
// caller.

/// A parsed enumerator's value: the integer where the expression evaluates
/// (so `1`, `0x1` and `1u << 0` compare equal), else its text (so two
/// unevaluable spellings still compare, conservatively).
#[derive(PartialEq, Eq)]
enum EnumVal {
    Num(i64),
    Text(String),
}

impl std::fmt::Display for EnumVal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnumVal::Num(n) => write!(f, "{n}"),
            EnumVal::Text(t) => f.write_str(t),
        }
    }
}

struct Field {
    /// The normalized declaration, e.g. `char message[256]`: the dimension is
    /// part of it, so resizing an array reads as a layout change.
    sig: String,
}

/// A `typedef struct {…} Name;` or `typedef enum {…} Name;`. Opaque handles
/// (`typedef struct Foo Foo;`) carry no layout and are not collected.
struct Aggregate {
    name: String,
    is_struct: bool,
    /// First field `uint32_t size`: the size-gated forward-compat marker.
    size_gated: bool,
    /// Marked `fig-abi: host-written`: fig allocates it, one at a time, and
    /// hands a callee a pointer to read, so an older callee reads the fields
    /// it knows at the offsets they had.
    host_written: bool,
    /// Marked `fig-abi: stride-gated`: only ever an element of an array whose
    /// holder states the stride it was written at (`FigNodeTable.row_size`,
    /// `FigLanguageVTable.dialect_size`), read prefix-wise.
    stride_gated: bool,
    fields: Vec<Field>,
    enumerators: Vec<(String, EnumVal)>,
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn read_ident(s: &str, i: usize) -> &str {
    let b = s.as_bytes();
    let mut e = i;
    while e < b.len() && is_name_char(b[e]) {
        e += 1;
    }
    &s[i..e]
}

/// The structs the header marks with `mark`: each mark names the `typedef
/// struct <Tag>` after it. Read off the raw header, since the mark is a
/// comment.
fn marked_names<'a>(header: &'a str, mark: &str) -> Vec<&'a str> {
    let mut names = Vec::new();
    let mut i = 0;
    while let Some(off) = header[i..].find(mark) {
        i += off + mark.len();
        let Some(td) = header[i..].find("typedef").map(|n| i + n) else {
            break;
        };
        let k = skip_ws(header.as_bytes(), td + "typedef".len());
        if read_ident(header, k) != "struct" {
            continue;
        }
        let tag = read_ident(header, skip_ws(header.as_bytes(), k + "struct".len()));
        if !tag.is_empty() {
            names.push(tag);
        }
    }
    names
}

fn collect_aggregates(header: &str) -> Vec<Aggregate> {
    let host_written = marked_names(header, "fig-abi: host-written");
    let stride_gated = marked_names(header, "fig-abi: stride-gated");
    let text = strip_directives(&strip_comments(header));
    let b = text.as_bytes();

    let mut list = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find("typedef") {
        let pos = i + off;
        let after_kw = pos + "typedef".len();
        // A keyword boundary on both sides.
        if (pos > 0 && is_name_char(b[pos - 1]))
            || (after_kw < b.len() && is_name_char(b[after_kw]))
        {
            i = after_kw;
            continue;
        }
        let kw_at = skip_ws(b, after_kw);
        let kw = read_ident(&text, kw_at);
        let is_struct = kw == "struct";
        if !is_struct && kw != "enum" {
            i = after_kw;
            continue;
        }
        let mut k = skip_ws(b, kw_at + kw.len());
        let tag = read_ident(&text, k);
        k = skip_ws(b, k + tag.len());
        if b.get(k) != Some(&b'{') {
            // Opaque or forward: no body. On to its `;`.
            let Some(semi) = text[k..].find(';') else {
                break;
            };
            i = k + semi + 1;
            continue;
        }
        // The brace body, nesting handled defensively (fig's do not nest).
        let mut depth = 0usize;
        let mut body_end = None;
        for (j, &c) in b.iter().enumerate().skip(k) {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        body_end = Some(j);
                        break;
                    }
                }
                _ => {}
            }
        }
        // Unbalanced: give up rather than misparse.
        let Some(body_end) = body_end else { break };
        let body = &text[k + 1..body_end];
        let alias_at = skip_ws(b, body_end + 1);
        let alias = read_ident(&text, alias_at);
        let name = if alias.is_empty() { tag } else { alias };
        i = text[alias_at..]
            .find(';')
            .map_or(text.len(), |n| alias_at + n + 1);
        if name.is_empty() {
            continue;
        }
        if is_struct {
            let fields = parse_fields(body);
            let size_gated = fields
                .first()
                .is_some_and(|f| field_name(&f.sig) == "size" && f.sig.starts_with("uint32_t"));
            list.push(Aggregate {
                name: name.to_string(),
                is_struct: true,
                size_gated,
                host_written: host_written.contains(&name),
                stride_gated: stride_gated.contains(&name),
                fields,
                enumerators: Vec::new(),
            });
        } else {
            list.push(Aggregate {
                name: name.to_string(),
                is_struct: false,
                size_gated: false,
                host_written: false,
                stride_gated: false,
                fields: Vec::new(),
                enumerators: parse_enumerators(body),
            });
        }
    }
    list
}

fn parse_fields(body: &str) -> Vec<Field> {
    body.split(';')
        .map(normalize_ws)
        .filter(|sig| !sig.is_empty() && !field_name(sig).is_empty())
        .map(|sig| Field { sig })
        .collect()
}

/// The identifier a normalized field declaration declares: the name run
/// ending at its first `[`, or else its trailing identifier.
fn field_name(sig: &str) -> &str {
    let b = sig.as_bytes();
    let end = match sig.find('[') {
        Some(br) => br,
        None => {
            let mut e = b.len();
            while e > 0 && !is_name_char(b[e - 1]) {
                e -= 1;
            }
            e
        }
    };
    let mut st = end;
    while st > 0 && is_name_char(b[st - 1]) {
        st -= 1;
    }
    &sig[st..end]
}

/// An enum body's enumerators with their values: explicit where written,
/// else C's running `previous + 1` from 0 — which stops being trackable past
/// an explicit value that does not evaluate.
fn parse_enumerators(body: &str) -> Vec<(String, EnumVal)> {
    let mut list = Vec::new();
    let mut running = Some(0i64);
    for raw in body.split(',') {
        let item = normalize_ws(raw);
        if item.is_empty() {
            continue;
        }
        let (name, value) = match item.split_once('=') {
            Some((name, expr)) => {
                let expr = expr.trim();
                match eval_enum(expr) {
                    Some(n) => {
                        running = Some(n + 1);
                        (name.trim(), EnumVal::Num(n))
                    }
                    None => {
                        running = None;
                        (name.trim(), EnumVal::Text(expr.to_string()))
                    }
                }
            }
            None => match running {
                Some(r) => {
                    running = Some(r + 1);
                    (item.as_str(), EnumVal::Num(r))
                }
                None => (item.as_str(), EnumVal::Text("<implicit>".into())),
            },
        };
        if !name.is_empty() {
            list.push((name.to_string(), value));
        }
    }
    list
}

/// The constant expressions fig's enumerators use: an integer literal
/// (decimal or `0x`, an optional u/U/l/L suffix, an optional sign) or one
/// `<<` shift of two. `None` for anything else.
fn eval_enum(expr: &str) -> Option<i64> {
    match expr.split_once("<<") {
        Some((l, r)) => {
            let (l, r) = (parse_lit(l)?, parse_lit(r)?);
            (0..=62).contains(&r).then(|| l << r)
        }
        None => parse_lit(expr),
    }
}

fn parse_lit(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest.trim()),
        None => (false, t.strip_prefix('+').unwrap_or(t).trim()),
    };
    let t = t.trim_end_matches(['u', 'U', 'l', 'L']);
    if t.is_empty() {
        return None;
    }
    let val = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() => i64::from_str_radix(hex, 16).ok()?,
        _ => t.parse().ok()?,
    };
    Some(if neg { -val } else { val })
}

fn diff_aggregates(base: &[Aggregate], cur: &[Aggregate]) -> (Verdict, Vec<String>) {
    let mut verdict = Verdict::Patch;
    let mut lines = Vec::new();
    for c in cur {
        let v = match base.iter().find(|b| b.name == c.name) {
            Some(b) if b.is_struct != c.is_struct => {
                lines.push(format!(
                    "~ {}: changed between struct and enum (MAJOR)",
                    c.name
                ));
                Verdict::Major
            }
            Some(b) if c.is_struct => diff_struct(&mut lines, b, c),
            Some(b) => diff_enum(&mut lines, b, c),
            None => {
                lines.push(format!("+ type {} added (MINOR)", c.name));
                Verdict::Minor
            }
        };
        verdict = verdict.max(v);
    }
    for b in base
        .iter()
        .filter(|b| !cur.iter().any(|c| c.name == b.name))
    {
        lines.push(format!("- type {} removed (MAJOR)", b.name));
        verdict = Verdict::Major;
    }
    (verdict, lines)
}

fn diff_enum(lines: &mut Vec<String>, b: &Aggregate, c: &Aggregate) -> Verdict {
    let mut v = Verdict::Patch;
    for (name, value) in &c.enumerators {
        match b.enumerators.iter().find(|(n, _)| n == name) {
            Some((_, was)) if was != value => {
                lines.push(format!(
                    "~ {}.{name}: enumerator value changed (was {was}, now {value}) (MAJOR)",
                    c.name
                ));
                v = Verdict::Major;
            }
            Some(_) => {}
            None => {
                lines.push(format!("+ {}.{name}: enumerator added (MINOR)", c.name));
                v = v.max(Verdict::Minor);
            }
        }
    }
    for (name, _) in &b.enumerators {
        if !c.enumerators.iter().any(|(n, _)| n == name) {
            lines.push(format!("- {}.{name}: enumerator removed (MAJOR)", c.name));
            v = Verdict::Major;
        }
    }
    v
}

fn diff_struct(lines: &mut Vec<String>, b: &Aggregate, c: &Aggregate) -> Verdict {
    let mut v = Verdict::Patch;
    for (i, (bf, cf)) in b.fields.iter().zip(&c.fields).enumerate() {
        if bf.sig != cf.sig {
            lines.push(format!(
                "~ {}: field {i} changed (was `{}`, now `{}`) (MAJOR)",
                c.name, bf.sig, cf.sig
            ));
            v = Verdict::Major;
        }
    }
    // Appending is safe only for a struct read prefix-wise. The marks are
    // promises about who allocates the struct or how it is strided, true of
    // the baseline too, so they are read off the current header alone.
    let gated = (b.size_gated && c.size_gated) || c.host_written || c.stride_gated;
    match c.fields.len().cmp(&b.fields.len()) {
        Ordering::Less => {
            lines.push(format!(
                "- {}: {} field(s) removed (MAJOR)",
                c.name,
                b.fields.len() - c.fields.len()
            ));
            v = Verdict::Major;
        }
        Ordering::Greater => {
            for f in &c.fields[b.fields.len()..] {
                if gated {
                    let why = if b.size_gated && c.size_gated {
                        "size-gated"
                    } else if c.host_written {
                        "host-written"
                    } else {
                        "stride-gated"
                    };
                    lines.push(format!(
                        "+ {}: field `{}` appended ({why}, MINOR)",
                        c.name, f.sig
                    ));
                    v = v.max(Verdict::Minor);
                } else {
                    lines.push(format!(
                        "+ {}: field `{}` added (not size-gated — changes layout, MAJOR)",
                        c.name, f.sig
                    ));
                    v = Verdict::Major;
                }
            }
        }
        Ordering::Equal => {}
    }
    v
}
