//! Port of src/diagnostics.zig.
//!
//! Serialization diagnostics: a read-only pass that reports what a conversion
//! would silently lose.
//!
//! fig's printers degrade or drop data whenever the target format can't hold a
//! value or a comment natively — a TOML `null` vanishes, a datetime collapses to
//! a string, a block comment becomes a run of `#` lines, plain JSON drops every
//! comment. None of that is an error (the output is still valid), so it happens
//! quietly. `analyze` surfaces it: it walks the AST exactly as a printer would
//! and returns one `Warning` per lossy event, each tagged with a `cause` so a
//! consumer (the CLI, a binding) can choose which to show.
//!
//! This is a separate pass, NOT instrumentation threaded through the printers:
//! the printers stay untouched, and the format-capability knowledge lives in one
//! place here (reusing `lossless::needs_envelope`/`is_unrepresentable` for the
//! formats whose capability the lossless layer already models). The walk shape
//! mirrors `lossless::lossy_strip` — same dotted/`[i]` path building — but emits
//! instead of removing.
//!
//! Scope is serialization only. Read-side lossiness (deserialize's
//! `ignore_unknown_fields`, YAML materialize dropping tags/anchors) is not
//! covered here; the model is additive if that is ever wanted.
//!
//! API:
//! ```text
//! pub type Target = serialize::Target;
//! pub struct Warning { pub code: Code, pub cause: Cause, pub path: Vec<u8>, pub note: &'static str }
//! impl Warning { pub fn render(&self, out: &mut Vec<u8>, fmt: &str) }
//! pub struct Options { pub pretty: bool, pub strip_comments: bool, pub lossless: bool, pub depth: usize }
//! pub fn analyze(ast: &AST, root_id: NodeId, target: &Target, options: &Options) -> Vec<Warning>;
//! pub enum Dropped { Null, Container }
//! pub fn first_dropped(ast: &AST, root_id: NodeId, target: &Target, depth: usize) -> Option<Dropped>;
//! ```

use crate::kit::ast::{ExtKind, NodeComments, NodeId, NodeKind, Style, AST};
use crate::kit::manifest::{NativeKinds, Prints, PrintsComments};
use crate::lossless;
use crate::registry::Entry;

/// What the tree is analyzed as being printed to: canonical, which holds
/// everything, or a registered language, whose losses are read off its
/// declaration.
pub use crate::serialize::Target;

/// One thing a conversion to a given format would lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: Code,
    pub cause: Cause,
    /// Dotted/`[i]` path to the affected node, relative to the analyzed root.
    /// Empty means the document root itself.
    pub path: Vec<u8>,
    /// Small extra context — for `TypeDegraded`, the type the value collapses
    /// to ("string", "number", …); otherwise empty.
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// A carried comment is not emitted at all (the target/mode has no comment
    /// syntax, or it was explicitly stripped).
    CommentDropped,
    /// A block comment is rendered as a run of line comments (the target has
    /// no block-comment syntax).
    CommentStyleDegraded,
    /// A node is removed entirely because the target cannot represent it even
    /// degraded (today: a `null` bound for TOML).
    ValueDropped,
    /// An `extended` or non-finite value is rendered as a poorer type (a
    /// datetime/enum as a string, a char as a number, `inf`/`nan` quoted).
    TypeDegraded,
}

/// Why the loss happens — so a consumer can keep or ignore each class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// The target format inherently cannot represent it.
    FormatLimitation,
    /// A caller option forced it (e.g. `strip_comments`).
    ExplicitOption,
}

impl Code {
    /// The Zig field name (`comment_dropped`), as the wire spells it.
    pub fn name(self) -> &'static str {
        match self {
            Code::CommentDropped => "comment_dropped",
            Code::CommentStyleDegraded => "comment_style_degraded",
            Code::ValueDropped => "value_dropped",
            Code::TypeDegraded => "type_degraded",
        }
    }
}

impl Cause {
    /// The Zig field name (`format_limitation`), as the wire spells it.
    pub fn name(self) -> &'static str {
        match self {
            Cause::FormatLimitation => "format_limitation",
            Cause::ExplicitOption => "explicit_option",
        }
    }
}

impl Warning {
    /// Write the default human-readable message (no trailing newline), for a
    /// target named `fmt` (`Target::name`). Bindings may instead render their
    /// own text from the structured fields.
    pub fn render(&self, out: &mut Vec<u8>, fmt: &str) {
        match self.code {
            Code::ValueDropped => {
                out.extend_from_slice(format!("dropped {} value at ", self.note).as_bytes());
                write_loc(out, &self.path);
                out.extend_from_slice(format!(" ({fmt} cannot represent it)").as_bytes());
            }
            Code::TypeDegraded => {
                out.extend_from_slice(b"degraded value at ");
                write_loc(out, &self.path);
                out.extend_from_slice(format!(" to {} ({fmt} has no native type for it)", self.note).as_bytes());
            }
            Code::CommentDropped => {
                if self.cause == Cause::ExplicitOption {
                    out.extend_from_slice(b"stripped comment at ");
                    write_loc(out, &self.path);
                } else {
                    out.extend_from_slice(b"dropped comment at ");
                    write_loc(out, &self.path);
                    out.extend_from_slice(format!(" ({fmt} has no comment syntax here)").as_bytes());
                }
            }
            Code::CommentStyleDegraded => {
                out.extend_from_slice(b"block comment at ");
                write_loc(out, &self.path);
                out.extend_from_slice(format!(" rendered as line comments ({fmt} has no block comments)").as_bytes());
            }
        }
    }
}

/// Write a node path for messages: backtick-quoted, or "the document root" for
/// the empty (root) path.
fn write_loc(out: &mut Vec<u8>, path: &[u8]) {
    if path.is_empty() {
        out.extend_from_slice(b"the document root");
    } else {
        out.push(b'`');
        out.extend_from_slice(path);
        out.push(b'`');
    }
}

/// What the consumer is asking the printer to do — mirrors the subset of
/// `SerializeOptions` that changes what gets lost, plus the lossless flag
/// (which lives outside `SerializeOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Pretty (multi-line) output. Comment-bearing formats with no compact
    /// comment form (JSON5/JSONC/ZON) drop comments when this is false.
    pub pretty: bool,
    /// The caller asked to drop comments. Every comment becomes a
    /// `CommentDropped`/`ExplicitOption` warning.
    pub strip_comments: bool,
    /// Lossless conversion is in effect: unrepresentable values are preserved
    /// through a `$fig` envelope, so value losses are suppressed. (Comment
    /// losses still apply — the envelope path still prints comments normally.)
    pub lossless: bool,
    /// How deep `root_id` will sit in the document it is printed into: 0 for
    /// a whole document, the path's length for a value spliced at a path. A
    /// format's depth limit counts from the document's root, not the
    /// value's, so a mapping that is fine as a dotenv document is dropped as
    /// a dotenv value.
    pub depth: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { pretty: true, strip_comments: false, lossless: false, depth: 0 }
    }
}

/// Walk the subtree rooted at `root_id` as it would be printed to `target`
/// and collect every lossy event. A language is held to what it declares,
/// which is what the CLI strips before printing to it: a `lossless`
/// declaration's missing kinds (a `null` dropped, an extended scalar
/// degraded), a `max_mapping_depth`'s flat-format drops, and its `syntax`'s
/// comment delimiters. What its printer does with a value, or a comment, it
/// declared nothing about is its own, and not reported.
pub fn analyze(ast: &AST, root_id: NodeId, target: &Target, options: &Options) -> Vec<Warning> {
    let mut c = Collector { ast, target, options: *options, warnings: Vec::new(), trail: Vec::new() };
    c.walk(root_id, options.depth);
    c.warnings
}

/// What `first_dropped` found: a `null` the target has no spelling for, or a
/// container it cannot hold at that depth (a sequence, a table too deep).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    Null,
    Container,
}

/// The first value in the subtree that `target` cannot hold at all once
/// spliced `depth` levels down — what an edit refuses rather than writing
/// text that reparses as something else (a mapping into dotenv as
/// `key=a=1`). `None` when every value has a spelling there, degraded or not.
pub fn first_dropped(ast: &AST, root_id: NodeId, target: &Target, depth: usize) -> Option<Dropped> {
    let warnings = analyze(ast, root_id, target, &Options { depth, strip_comments: true, ..Options::default() });
    for w in &warnings {
        if w.code == Code::ValueDropped {
            return Some(if w.note == "null" { Dropped::Null } else { Dropped::Container });
        }
    }
    None
}

#[derive(Debug, Clone, Copy)]
enum Segment {
    Key(NodeId),
    Index(usize),
}

struct Collector<'a> {
    ast: &'a AST,
    target: &'a Target,
    options: Options,
    warnings: Vec<Warning>,
    /// The path to the node being visited, a segment per level. Spelled
    /// out (`here`) only for a warning, since most walks raise none.
    trail: Vec<Segment>,
}

impl Collector<'_> {
    fn add(&mut self, code: Code, cause: Cause, note: &'static str) {
        let path = self.here();
        self.warnings.push(Warning { code, cause, path, note });
    }

    /// `trail` as a path: `a.b[1].c`, or empty at the root. A non-string
    /// key shows as `?` (only its value can be a loss target, so the key
    /// text is a hint).
    fn here(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, seg) in self.trail.iter().enumerate() {
            match *seg {
                Segment::Key(id) => {
                    if i > 0 {
                        out.push(b'.');
                    }
                    let name: &[u8] = match self.ast.kind_of(id) {
                        NodeKind::String(x) => x,
                        _ => b"?",
                    };
                    out.extend_from_slice(name);
                }
                Segment::Index(n) => out.extend_from_slice(format!("[{n}]").as_bytes()),
            }
        }
        out
    }

    /// Visit one node: check its own value + comments, then recurse into a
    /// container's children (building each child's path). `depth` is the
    /// node's depth in the printed document (`Options.depth` for the node
    /// `analyze` was called with), incrementing by one per container level
    /// — used by `value_loss`'s flat-format arm, whose capability depends on
    /// nesting depth (root + one level of `[section]`s are fine; deeper is
    /// not) and not just root-vs-not like every other format needs.
    fn walk(&mut self, id: NodeId, depth: usize) {
        self.check_comments(id);
        self.check_value(id, depth);
        match self.ast.kind_of(id) {
            NodeKind::Mapping(first) => {
                let mut child = first;
                while let Some(cid) = child {
                    let kv = self.ast.kind_of(cid).as_keyvalue().expect("a mapping child is a keyvalue");
                    self.trail.push(Segment::Key(kv.key));
                    // A mapping entry's leading comment binds to its KEY node; its
                    // trailing/value losses ride the value node (checked in walk).
                    self.check_comments(kv.key);
                    self.walk(kv.value, depth + 1);
                    self.trail.pop();
                    child = self.ast.node(cid).next_sibling;
                }
            }
            NodeKind::Sequence(first) => {
                let mut child = first;
                let mut i = 0usize;
                while let Some(cid) = child {
                    self.trail.push(Segment::Index(i));
                    i += 1;
                    self.walk(cid, depth + 1);
                    self.trail.pop();
                    child = self.ast.node(cid).next_sibling;
                }
            }
            _ => {}
        }
    }

    fn check_value(&mut self, id: NodeId, depth: usize) {
        let kind = self.ast.kind_of(id);
        let loss = match self.target {
            Target::Canonical => None,
            Target::Language(e) => value_loss(e, &kind, depth),
        };
        let Some(loss) = loss else { return };
        // Under lossless, unrepresentable values are enveloped, not lost.
        if self.options.lossless {
            return;
        }
        self.add(loss.code, Cause::FormatLimitation, loss.note);
    }

    fn check_comments(&mut self, id: NodeId) {
        let nc = self.ast.comments(id);
        if nc.is_empty() {
            return;
        }
        // An explicit `strip_comments` is the user asking for the drop: that's the
        // actionable reason, so it wins even when the target also couldn't hold
        // the comment (a consumer filtering out `ExplicitOption` then correctly
        // stays silent — it's not a surprising loss).
        if self.options.strip_comments {
            self.add(Code::CommentDropped, Cause::ExplicitOption, "");
            return;
        }
        // Otherwise, a format with no comment syntax here drops it outright.
        let (emitted, block) = match self.target {
            Target::Canonical => return,
            // A language that declares no `syntax` has said nothing about
            // comments, and its printer receives every comment row, so there
            // is no loss to claim.
            Target::Language(e) => match declared_comments(e, self.options.pretty) {
                Some(x) => x,
                None => return,
            },
        };
        if !emitted {
            self.add(Code::CommentDropped, Cause::FormatLimitation, "");
            return;
        }
        if !block && has_block(&nc) {
            self.add(Code::CommentStyleDegraded, Cause::FormatLimitation, "");
        }
    }
}

// ── Capability table ────────────────────────────────────────────────────────
//
// The single source of truth for what each target format can hold, read off
// the dialect's `prints` declaration where it makes one, else off the
// language's `caps.lossless` and `caps.max_mapping_depth` (delegated to
// `lossless::needs_envelope`/`is_unrepresentable`).

struct Loss {
    code: Code,
    note: &'static str,
}

/// The loss `prints` and `max_mapping_depth` say serializing `kind` at
/// `depth` causes. A flat format (one with `max_mapping_depth`) holds no
/// sequence, and no mapping past that depth.
fn declared_loss(p: &Prints, max_depth: Option<u8>, kind: &NodeKind<'_>, depth: usize) -> Option<Loss> {
    if p.root_container && depth == 0 {
        match kind {
            NodeKind::Mapping(_) | NodeKind::Sequence(_) => {}
            _ => return Some(Loss { code: Code::ValueDropped, note: root_scalar_note(kind) }),
        }
    }
    match kind {
        NodeKind::Null => {
            if !p.holds.null && !(p.null_at_root && depth == 0) {
                return Some(Loss { code: Code::ValueDropped, note: "null" });
            }
        }
        NodeKind::Boolean(_) | NodeKind::Number(_) => {
            if !p.typed_scalars {
                return Some(Loss { code: Code::TypeDegraded, note: "string" });
            }
        }
        NodeKind::Extended(e) => {
            if !holds_ext(&p.holds, e.kind) {
                return Some(Loss { code: Code::TypeDegraded, note: degraded_note(p, e.kind) });
            }
        }
        NodeKind::Sequence(_) => {
            if max_depth.is_some() {
                return Some(Loss { code: Code::ValueDropped, note: "array" });
            }
        }
        NodeKind::Mapping(_) => {
            if let Some(m) = max_depth {
                if depth > m as usize {
                    return Some(Loss { code: Code::ValueDropped, note: "table" });
                }
            }
        }
        _ => {}
    }
    None
}

fn holds_ext(holds: &NativeKinds, ext: ExtKind) -> bool {
    holds.ext(ext)
}

/// The value loss printing `kind` at `depth` to `e` would cause, or `None`,
/// from its declaration alone — the same two declarations the CLI strips by,
/// so the warning and the strip cannot disagree. `lossless` names the kinds
/// it holds: a `null` it lacks is dropped, an extended scalar it lacks
/// reaches its printer as text. `max_mapping_depth` makes it a flat format:
/// no `null`, no sequence, no mapping past the depth. Typed scalars are not
/// reported degraded, as a compiled flat format's are, because nothing in
/// the declaration says so.
fn value_loss(e: &Entry, kind: &NodeKind<'_>, depth: usize) -> Option<Loss> {
    let caps = e.language.caps;
    if let Some(p) = &e.prints {
        return declared_loss(p, caps.max_mapping_depth, kind, depth);
    }
    if let Some(native) = caps.lossless {
        if lossless::is_unrepresentable(native, kind) {
            return Some(Loss { code: Code::ValueDropped, note: drop_note(kind) });
        }
        if lossless::needs_envelope(native, kind) {
            if let NodeKind::Extended(_) = kind {
                return Some(Loss { code: Code::TypeDegraded, note: "string" });
            }
        }
    }
    if let Some(max) = caps.max_mapping_depth {
        match kind {
            NodeKind::Null => return Some(Loss { code: Code::ValueDropped, note: "null" }),
            NodeKind::Sequence(_) => return Some(Loss { code: Code::ValueDropped, note: "array" }),
            NodeKind::Mapping(_) => {
                if depth > max as usize {
                    return Some(Loss { code: Code::ValueDropped, note: "table" });
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether a language writes comments at all, and whether it has a
/// spelling for a block one: its dialect's `prints.comments` where it
/// declares one, else its `syntax`'s own-line delimiter; a block spelling
/// is a delimiter that is a pair (`<!-- -->`) and can hold several lines.
/// `None` when it declares neither, since a language may print every comment
/// it is handed and fig cannot know that it does not.
fn declared_comments(e: &Entry, pretty: bool) -> Option<(bool, bool)> {
    let block = match &e.syntax {
        Some(syntax) => syntax.comments.block.is_some() || syntax.comments.line.is_some_and(|l| !l.close.is_empty()),
        None => false,
    };
    // What the printer says it writes decides, with or without a `syntax`.
    if let Some(p) = &e.prints {
        let emitted = match p.comments {
            PrintsComments::Always => true,
            PrintsComments::Pretty => pretty,
            PrintsComments::Never => false,
        };
        return Some((emitted, block));
    }
    let syntax = e.syntax.as_ref()?;
    if syntax.comments.line.is_none() {
        return Some((false, false));
    }
    Some((true, block))
}

/// The type an `extended` value the target does not hold collapses to, as
/// its printer writes it: a char literal and a non-finite float as the
/// dialect declares, every other kind as a string.
fn degraded_note(p: &Prints, ext: ExtKind) -> &'static str {
    match ext {
        ExtKind::CharLiteral => p.char_as,
        ExtKind::NumberSpecial => p.number_special_as,
        _ => "string",
    }
}

/// Describes `kind` for the `ValueDropped` note when it is a dialect's
/// root-position loss (`Prints.root_container`, figl's). Never called for
/// `Mapping`/`Sequence` (the caller already excludes those) or the
/// container-only `KeyValue` kind, which never reaches a value-loss check.
fn root_scalar_note(kind: &NodeKind<'_>) -> &'static str {
    match kind {
        NodeKind::Null => "null",
        NodeKind::Boolean(_) => "boolean",
        NodeKind::Number(_) => "number",
        NodeKind::String(_) => "string",
        NodeKind::Alias(_) => "alias",
        NodeKind::Extended(e) => match e.kind {
            ExtKind::EnumLiteral => "enum",
            ExtKind::CharLiteral => "char",
            ExtKind::NumberSpecial => "number",
            ExtKind::OffsetDatetime | ExtKind::LocalDatetime | ExtKind::LocalDate | ExtKind::LocalTime => "datetime",
            ExtKind::PlistDate => "date",
            ExtKind::Data => "data",
        },
        NodeKind::Mapping(_) | NodeKind::Sequence(_) | NodeKind::KeyValue(_) => unreachable!(),
    }
}

fn drop_note(kind: &NodeKind<'_>) -> &'static str {
    match kind {
        NodeKind::Null => "null",
        _ => "",
    }
}

/// Whether any of `nc` is a block comment.
fn has_block(nc: &NodeComments<'_>) -> bool {
    nc.leading.iter().any(|c| c.style == Style::Block)
        || nc.dangling.iter().any(|c| c.style == Style::Block)
        || nc.trailing.is_some_and(|t| t.style == Style::Block)
}

// ── Tests ───────────────────────────────────────────────────────────────────
//
// Most name a bundled dialect (`toml`, `json`, `json5`, `yaml`, `figl`,
// `dotenv`, `ini`) and return early while the global registry has none
// (a test build's global registry holds fig's ten,
// from the language crates). The
// registered-target test builds its own entry over a test language.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{Builder, Comment, Entry as MapEntry};
    use crate::kit::manifest::{Comments, SpliceStyle, Syntax};
    use crate::registry::{self, Registered};
    use std::borrow::Cow;
    use std::sync::Arc;

    /// The registered dialect `name`, or `None` before it is bundled.
    fn target(name: &str) -> Option<Target> {
        Target::named_in(registry::global(), name)
    }

    /// Build a small AST via the Builder and run `analyze`, or `None` when
    /// `name` is not registered.
    fn analyze_built(build: fn(&mut Builder) -> NodeId, name: &str, options: Options) -> Option<Vec<Warning>> {
        let t = target(name)?;
        let mut b = Builder::init();
        let root = build(&mut b);
        let ast = b.finish(root).unwrap();
        Some(analyze(&ast, ast.root, &t, &options))
    }

    fn null_entry(b: &mut Builder) -> NodeId {
        let k = b.add_string(b"a").unwrap();
        let v = b.add_null().unwrap();
        b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap()
    }

    #[test]
    fn null_dropped_for_toml_not_for_json_yaml() {
        if let Some(toml) = analyze_built(null_entry, "toml", Options::default()) {
            assert_eq!(1, toml.len());
            assert_eq!(Code::ValueDropped, toml[0].code);
            assert_eq!(b"a", &toml[0].path[..]);
        }
        if let Some(json) = analyze_built(null_entry, "json", Options::default()) {
            assert_eq!(0, json.len());
        }
    }

    #[test]
    fn datetime_degrades_to_json_canonical_to_toml() {
        fn f(b: &mut Builder) -> NodeId {
            b.add_extended(ExtKind::OffsetDatetime, b"1979-05-27T07:32:00Z").unwrap()
        }
        if let Some(json) = analyze_built(f, "json", Options::default()) {
            assert_eq!(1, json.len());
            assert_eq!(Code::TypeDegraded, json[0].code);
            assert_eq!("string", json[0].note);
        }
        if let Some(toml) = analyze_built(f, "toml", Options::default()) {
            assert_eq!(0, toml.len());
        }
    }

    #[test]
    fn non_finite_float_degrades_to_json_but_not_json5() {
        fn f(b: &mut Builder) -> NodeId {
            b.add_extended(ExtKind::NumberSpecial, b"Infinity").unwrap()
        }
        if let Some(json) = analyze_built(f, "json", Options::default()) {
            assert_eq!(1, json.len());
            assert_eq!("quoted string", json[0].note);
        }
        if let Some(json5) = analyze_built(f, "json5", Options::default()) {
            assert_eq!(0, json5.len());
        }
    }

    #[test]
    fn char_literal_is_native_to_fig_degrades_to_a_number_elsewhere() {
        // Wrapped in a mapping (not bare at the root) so this exercises the
        // char-literal KIND's capability, not fig's separate root-position rule.
        fn f(b: &mut Builder) -> NodeId {
            let k = b.add_string(b"a").unwrap();
            let v = b.add_extended(ExtKind::CharLiteral, b"65").unwrap(); // 'A'
            b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap()
        }
        // fig re-emits it via `: char =` — no loss.
        if let Some(fig) = analyze_built(f, "figl", Options::default()) {
            assert_eq!(0, fig.len());
        }
        // JSON has no char; it collapses to the codepoint as a number.
        if let Some(json) = analyze_built(f, "json", Options::default()) {
            assert_eq!(1, json.len());
            assert_eq!(Code::TypeDegraded, json[0].code);
            assert_eq!("number", json[0].note);
        }
    }

    #[test]
    fn fig_root_scalar_is_flagged_as_unrepresentable_a_root_map_sequence_is_not() {
        fn scalar_root(b: &mut Builder) -> NodeId {
            b.add_number_raw(b"42", false).unwrap()
        }
        if let Some(scalar) = analyze_built(scalar_root, "figl", Options::default()) {
            assert_eq!(1, scalar.len());
            assert_eq!(Code::ValueDropped, scalar[0].code);
            assert_eq!(Cause::FormatLimitation, scalar[0].cause);
            assert!(scalar[0].path.is_empty());
            assert_eq!("number", scalar[0].note);
        }
        // Other formats have no such rule — a bare JSON number root is fine.
        if let Some(json) = analyze_built(scalar_root, "json", Options::default()) {
            assert_eq!(0, json.len());
        }
        // A root mapping is unaffected — still fully representable.
        fn map_root(b: &mut Builder) -> NodeId {
            let k = b.add_string(b"a").unwrap();
            let v = b.add_number_raw(b"1", false).unwrap();
            b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap()
        }
        if let Some(map_fig) = analyze_built(map_root, "figl", Options::default()) {
            assert_eq!(0, map_fig.len());
        }
    }

    #[test]
    fn lossless_suppresses_value_warnings() {
        if let Some(toml) = analyze_built(null_entry, "toml", Options { lossless: true, ..Options::default() }) {
            assert_eq!(0, toml.len());
        }
    }

    #[test]
    fn comment_dropped_style_degraded_and_explicitly_stripped() {
        fn f(b: &mut Builder) -> NodeId {
            let k = b.add_string(b"a").unwrap();
            let v = b.add_number_raw(b"1", false).unwrap();
            let m = b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap();
            // A block comment leads the entry (binds to the key node).
            b.set_comments(k, &NodeComments { leading: Cow::Owned(vec![Comment { text: b"note", style: Style::Block }]), ..Default::default() })
                .unwrap();
            m
        }
        // YAML emits comments but has no block syntax → style degraded.
        if let Some(yaml) = analyze_built(f, "yaml", Options::default()) {
            assert_eq!(1, yaml.len());
            assert_eq!(Code::CommentStyleDegraded, yaml[0].code);
            assert_eq!(b"a", &yaml[0].path[..]);
        }
        // Plain JSON has no comments at all → dropped, format limitation.
        if let Some(json) = analyze_built(f, "json", Options::default()) {
            assert_eq!(1, json.len());
            assert_eq!(Code::CommentDropped, json[0].code);
            assert_eq!(Cause::FormatLimitation, json[0].cause);
        }
        // JSON5 keeps block comments → no warning.
        if let Some(json5) = analyze_built(f, "json5", Options::default()) {
            assert_eq!(0, json5.len());
        }
        // Explicit strip on a format that would otherwise keep it → explicit_option.
        if let Some(stripped) = analyze_built(f, "json5", Options { strip_comments: true, ..Options::default() }) {
            assert_eq!(1, stripped.len());
            assert_eq!(Code::CommentDropped, stripped[0].code);
            assert_eq!(Cause::ExplicitOption, stripped[0].cause);
        }
        // Explicit strip wins even when the target ALSO can't hold comments.
        if let Some(stripped_json) = analyze_built(f, "json", Options { strip_comments: true, ..Options::default() }) {
            assert_eq!(1, stripped_json.len());
            assert_eq!(Cause::ExplicitOption, stripped_json[0].cause);
        }
    }

    #[test]
    fn a_registered_target_is_warned_about_comments_by_what_it_declares() {
        let mut b = Builder::init();
        let k = b.add_string(b"a").unwrap();
        let v = b.add_number_raw(b"1", false).unwrap();
        let m = b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap();
        b.set_comments(k, &NodeComments { leading: Cow::Owned(vec![Comment { text: b"note", style: Style::Line }]), ..Default::default() })
            .unwrap();
        let ast = b.finish(m).unwrap();

        let reg = Arc::new(Registered {
            lang: &registry::tests::TINY,
            name: "t".into(),
            caps: crate::kit::manifest::Caps { serialize: true, ..Default::default() },
            syntax: None,
            samples: Vec::new(),
            first_index: 0,
            entry_count: 1,
        });
        let entry = |syntax: Option<Syntax>, prints: Option<Prints>| {
            Target::Language(Arc::new(Entry {
                index: 0,
                abi: 0,
                language: reg.clone(),
                name: "t".into(),
                dialect: 0,
                print_dialect: 0,
                extensions: Vec::new(),
                splice: SpliceStyle::Literal,
                empty_doc_seed: None,
                syntax,
                prints,
            }))
        };

        // No `syntax`: nothing said about comments, and the printer is handed
        // every one — so no loss is claimed.
        assert_eq!(0, analyze(&ast, ast.root, &entry(None, None), &Options::default()).len());

        // A `syntax` with no comment delimiter declares there are none: dropped.
        let dropped = analyze(&ast, ast.root, &entry(Some(Syntax::new(Comments::NONE, None, None)), None), &Options::default());
        assert_eq!(1, dropped.len());
        assert_eq!(Code::CommentDropped, dropped[0].code);

        // One with a delimiter writes them: no warning.
        assert_eq!(0, analyze(&ast, ast.root, &entry(Some(Syntax::new(Comments::HASH, None, None)), None), &Options::default()).len());

        // A dialect whose `prints` says it never writes comments drops them,
        // whether or not the language declares a `syntax` (a read-and-print one
        // need not).
        let prints = Prints {
            comments: PrintsComments::Never,
            ..Prints::new(NativeKinds { null: true, ..NativeKinds::NONE })
        };
        let never = analyze(&ast, ast.root, &entry(None, Some(prints)), &Options::default());
        assert_eq!(1, never.len());
        assert_eq!(Code::CommentDropped, never[0].code);
    }

    #[test]
    fn path_shapes_match_lossy_strip() {
        // { b: null, c: [1, null], d: { e: null } } → toml
        fn f(b: &mut Builder) -> NodeId {
            let bk = b.add_string(b"b").unwrap();
            let bv = b.add_null().unwrap();
            let c1 = b.add_number_raw(b"1", false).unwrap();
            let c2 = b.add_null().unwrap();
            let cseq = b.add_sequence(&[c1, c2]).unwrap();
            let ck = b.add_string(b"c").unwrap();
            let ek = b.add_string(b"e").unwrap();
            let ev = b.add_null().unwrap();
            let inner = b.add_mapping(&[MapEntry { key: ek, value: ev }]).unwrap();
            let dk = b.add_string(b"d").unwrap();
            b.add_mapping(&[
                MapEntry { key: bk, value: bv },
                MapEntry { key: ck, value: cseq },
                MapEntry { key: dk, value: inner },
            ])
            .unwrap()
        }
        if let Some(w) = analyze_built(f, "toml", Options::default()) {
            assert_eq!(3, w.len());
            assert_eq!(b"b", &w[0].path[..]);
            assert_eq!(b"c[1]", &w[1].path[..]);
            assert_eq!(b"d.e", &w[2].path[..]);
        }
    }

    #[test]
    fn a_depth_limit_counts_from_the_documents_root_not_the_analyzed_values() {
        let (Some(dotenv), Some(ini)) = (target("dotenv"), target("ini")) else { return };
        let mut b = Builder::init();
        let k = b.add_string(b"x").unwrap();
        let v = b.add_string(b"1").unwrap();
        let map = b.add_mapping(&[MapEntry { key: k, value: v }]).unwrap();
        let ast = b.finish(map).unwrap();

        // As a whole dotenv document the mapping is the root, and fine; as a
        // value one level down it is a table dotenv cannot hold.
        assert_eq!(0, analyze(&ast, ast.root, &dotenv, &Options::default()).len());
        let nested = analyze(&ast, ast.root, &dotenv, &Options { depth: 1, ..Options::default() });
        assert_eq!(1, nested.len());
        assert_eq!(Code::ValueDropped, nested[0].code);

        // INI holds one level of `[section]`s, so the same value is fine one
        // level down and dropped two.
        assert_eq!(None, first_dropped(&ast, ast.root, &ini, 1));
        assert_eq!(Some(Dropped::Container), first_dropped(&ast, ast.root, &ini, 2));
        // A degraded value is not a dropped one: dotenv writes the number as text.
        assert_eq!(None, first_dropped(&ast, v, &dotenv, 1));
    }

    #[test]
    fn first_dropped_tells_a_null_from_a_container() {
        let mut b = Builder::init();
        let n = b.add_null().unwrap();
        let ast = b.finish(n).unwrap();
        if let Some(toml) = target("toml") {
            assert_eq!(Some(Dropped::Null), first_dropped(&ast, ast.root, &toml, 1));
        }
        if let Some(yaml) = target("yaml") {
            assert_eq!(None, first_dropped(&ast, ast.root, &yaml, 1));
        }
    }

    #[test]
    fn a_warning_renders_its_default_message() {
        let mut out = Vec::new();
        Warning { code: Code::ValueDropped, cause: Cause::FormatLimitation, path: b"a.b".to_vec(), note: "null" }.render(&mut out, "toml");
        assert_eq!("dropped null value at `a.b` (toml cannot represent it)", String::from_utf8_lossy(&out));
        out.clear();
        Warning { code: Code::TypeDegraded, cause: Cause::FormatLimitation, path: Vec::new(), note: "string" }.render(&mut out, "json");
        assert_eq!("degraded value at the document root to string (json has no native type for it)", String::from_utf8_lossy(&out));
        out.clear();
        Warning { code: Code::CommentDropped, cause: Cause::ExplicitOption, path: b"[0]".to_vec(), note: "" }.render(&mut out, "json");
        assert_eq!("stripped comment at `[0]`", String::from_utf8_lossy(&out));
        out.clear();
        Warning { code: Code::CommentDropped, cause: Cause::FormatLimitation, path: b"k".to_vec(), note: "" }.render(&mut out, "json");
        assert_eq!("dropped comment at `k` (json has no comment syntax here)", String::from_utf8_lossy(&out));
        out.clear();
        Warning { code: Code::CommentStyleDegraded, cause: Cause::FormatLimitation, path: b"k".to_vec(), note: "" }.render(&mut out, "yaml");
        assert_eq!("block comment at `k` rendered as line comments (yaml has no block comments)", String::from_utf8_lossy(&out));
    }

    #[test]
    fn canonical_holds_everything() {
        let mut b = Builder::init();
        let root = null_entry(&mut b);
        b.set_comments(root, &NodeComments { leading: Cow::Owned(vec![Comment { text: b"c", style: Style::Block }]), ..Default::default() })
            .unwrap();
        let ast = b.finish(root).unwrap();
        assert!(analyze(&ast, ast.root, &Target::Canonical, &Options::default()).is_empty());
        let stripped = analyze(&ast, ast.root, &Target::Canonical, &Options { strip_comments: true, ..Options::default() });
        assert_eq!(1, stripped.len());
        assert_eq!(Cause::ExplicitOption, stripped[0].cause);
    }
}
