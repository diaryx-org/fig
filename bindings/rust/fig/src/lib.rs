// Lets the `derive`-generated code refer to this crate as `fig::…` even from
// within the crate's own tests and examples.
extern crate self as fig;

mod diagnostics;
mod editor;
mod error;
mod language_crate;
mod value;

// The core is `fig-core`, in Rust. Its `api` is fig's C ABI less the
// marshalling — the same calls under the same names, the same argument
// checks and the same statuses — so this crate reads as it did over
// `libfig`: `fig_parse_ex` is `capi::Document::parse_ex`, and so on.
pub(crate) use fig_core::api as capi;

#[cfg(feature = "derive")]
mod convert;
#[cfg(feature = "serde")]
mod de;
#[cfg(feature = "serde")]
mod ser;

use std::os::raw::c_int;

pub use diagnostics::{Warning, WarningCause, WarningCode};
pub use editor::{Editor, Segment};

/// The out-of-process carrier of a runtime language: JSON-RPC over stdio,
/// read and written with `serde_json`. With the `helper` feature.
#[cfg(feature = "helper")]
pub mod helper;
/// The kit's number and datetime rules, which fig's own languages are
/// written with.
pub mod kit;
pub mod language;
#[cfg(test)]
mod test_languages;
pub use error::{Error, LanguageFailure, ParseError};
pub use value::{ExtKind, Value};

#[cfg(feature = "derive")]
pub use convert::{FromValue, ToValue};
// Shared helpers the derive macros call instead of inlining a lookup, convert,
// and error per field.
#[cfg(feature = "derive")]
#[doc(hidden)]
pub use convert::{field, field_or_default, map_get};
// The derive macros share the trait names (trait vs. macro namespace), mirroring
// `serde::Serialize`. Glob users get both with one import.
#[cfg(feature = "derive")]
pub use fig_macros::{FromValue, ToValue};

#[cfg(feature = "serde")]
pub use de::from_slice;
#[cfg(feature = "serde")]
pub use ser::to_value;

/// A node of a parsed document, as the core numbers them.
pub(crate) type FigNodeId = fig_core::kit::ast::NodeId;
pub(crate) use capi::{FigNodeKind, NODE_NONE as FIG_NODE_NONE};

/// A half-open `[start, end)` byte range within the input, as a runtime
/// language's node table records one. Converts to and from a
/// `Range<usize>`, so `Span::from(0..5)` builds one and
/// `&source[Range::from(span)]` slices by one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl From<std::ops::Range<usize>> for Span {
    fn from(r: std::ops::Range<usize>) -> Self {
        Span {
            start: r.start,
            end: r.end,
        }
    }
}

impl From<Span> for std::ops::Range<usize> {
    fn from(s: Span) -> Self {
        s.start..s.end
    }
}

/// A config format: an opaque, per-process handle naming a dialect
/// registered with fig's core.
///
/// A `Format` is obtained by name — [`Format::by_name`]`("yaml")` — or from
/// [`language::register`] and [`Format::detect`]; it cannot be built from an
/// integer. Handles are assigned in registration
/// order and differ between processes: persist [`Format::name`] and resolve it
/// again with [`Format::by_name`], never the handle.
///
/// The core carries no language: each is a crate of its own
/// (`fig-yaml`, …) whose `register()` returns its first dialect's format, and
/// until a language is registered [`Format::by_name`] returns `None` for its
/// names. Besides the dialect names, a language's `--spec` versions are
/// dialects of their own (`yaml-1.1`, `yaml-1.2.2`, `toml-1.0`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Format(c_int);

impl Format {
    /// The C ABI handle.
    pub(crate) fn to_c(self) -> c_int {
        self.0
    }

    /// The `Format` an ABI handle names, or `None` for one never handed out.
    pub(crate) fn from_c(value: c_int) -> Option<Format> {
        if value <= 0 || capi::format_name(value).is_none() {
            return None;
        }
        Some(Format(value))
    }

    /// The registered format named `name` (`"json5"`, `"yaml"`, `"yaml-1.1"`,
    /// or a language registered at runtime). `None` when nothing answers to it
    /// in this process — a misspelling, or a language this build compiled out.
    pub fn by_name(name: &str) -> Option<Format> {
        // A name is a C string at the ABI: one holding a NUL names nothing.
        if name.contains('\0') {
            return None;
        }
        let value = capi::format_by_name(name);
        if value <= 0 {
            return None;
        }
        Some(Format(value))
    }

    /// The dialect name this format was registered under — the name
    /// [`Format::by_name`] resolves back to this handle.
    ///
    /// Every `Format` this crate hands out names a registered dialect, and
    /// the core never forgets one, so the name is always there; should the
    /// core ever answer with none (or with a name that is not UTF-8), this is
    /// the empty string, which [`Format::by_name`] resolves to nothing.
    pub fn name(self) -> &'static str {
        format_name_static(self.0)
    }

    /// The documents of a multi-document stream in this format (YAML's
    /// `---`/`...`), each a byte range of `input` to parse as a document of
    /// its own. An empty stream is one document.
    /// [`Error::UnsupportedOperation`] for a format without streams.
    pub fn split_stream(self, input: &[u8]) -> Result<Vec<language::StreamDocument>, Error> {
        match capi::stream_split(input, self.to_c()) {
            Ok(docs) => Ok(docs
                .into_iter()
                .map(|d| language::StreamDocument {
                    start: d.start,
                    end: d.end,
                    explicit: d.marked,
                })
                .collect()),
            Err(e) => Err(Error::from_fig_error(&e)),
        }
    }

    /// The first format `order` names whose parser accepts `input`, tried in
    /// that order: content sniffing, with the order the caller's to choose.
    /// The grammars overlap, so put the strictest first. A name nothing
    /// answers to is skipped.
    pub fn detect(input: &[u8], order: &[&str]) -> Option<Format> {
        // A name is a C string at the ABI: one holding a NUL is dropped.
        let names: Vec<&str> = order.iter().copied().filter(|n| !n.contains('\0')).collect();
        let out = capi::detect_order(input, &names).ok()?;
        Format::from_c(out)
    }
}

/// The name the registry holds for format `abi`, for the life of the
/// process: the registry is append-only, so each name is copied out once and
/// kept. The empty string for a format never handed out.
fn format_name_static(abi: c_int) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashMap<c_int, &'static str>>> = OnceLock::new();
    let names = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut names = names.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(name) = names.get(&abi) {
        return name;
    }
    let Some(name) = capi::format_name(abi) else {
        return "";
    };
    let name: &'static str = Box::leak(name.into_boxed_str());
    names.insert(abi, name);
    name
}

/// Controls how [`Value::serialize_with`] renders output. The [`Default`] is
/// fig's historical style (pretty-printed, two-space indent), so
/// [`Value::serialize`] is exactly `serialize_with(format, SerializeOptions::default())`.
///
/// Each language reads the options it has a use for. Of the bundled ones,
/// `pretty` is honored by `json` (multi-line vs. minified), `zon` (`zig fmt`
/// multi-line vs. inline `.{ a, b }`), and `toml` (gates array wrapping);
/// `indent` by `json` and `toml`'s wrapped arrays; `width` by the
/// inline-vs-expanded (flow vs. block) layout of `toml`, `yaml`, and `fig` —
/// with the caveats on [`width`](SerializeOptions::width) itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SerializeOptions {
    /// `true`: multi-line, indented output. `false`: compact single-line output
    /// with no insignificant whitespace. For TOML, `false` keeps every array on
    /// one line; `true` lets a wide array wrap (see `width`).
    pub pretty: bool,
    /// Spaces per indentation level when `pretty` is set (JSON, and TOML's wrapped
    /// arrays).
    pub indent: u8,
    /// Drop comments carried on the value instead of emitting them. Default
    /// `false` (preserve them where the target format allows).
    pub strip_comments: bool,
    /// [`Document::serialize`] only: preserve values the target format cannot
    /// represent natively (a null in TOML, a TOML datetime in JSON, …) through a
    /// `$fig` envelope, and decode any such envelope found in the source. Default
    /// `false` (lossy — an unrepresentable value is an [`Error::UnsupportedFormat`]).
    /// Ignored by [`Value::serialize_with`] (a built value has no source envelopes).
    pub lossless: bool,
    /// The column budget for the inline-vs-expanded layout of `toml`, `yaml`,
    /// and `fig` (of the bundled languages). A mapping/array that renders within
    /// `width` columns stays inline (flow: `k = { … }` / `[a, b]`); a wider one
    /// expands to a `[section]`, a wrapped array, or block form. Default `80`.
    /// Ignored by the other formats.
    ///
    /// Two limits to know before reaching for this as a "render block" lever:
    ///
    /// * **`0` means "unset", not "never inline".** It resolves to the default
    ///   `80` at the C ABI, where zero-initialized option structs are ordinary.
    ///   Pass `1` to force block layout.
    /// * **For YAML the budget applies to nested containers only.** A *root*
    ///   map or sequence always renders block, whatever the width — so
    ///   `Value::Map([("k", "v")]).serialize_with(yaml, …)` is `"k: v\n"` at
    ///   every setting, and a width change first becomes visible one level in
    ///   (`a: { k: v }` vs `a:\n  k: v`).
    pub width: u16,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self {
            pretty: true,
            indent: 2,
            strip_comments: false,
            lossless: false,
            width: 80,
        }
    }
}

/// `SerializeOptions` is `#[non_exhaustive]`, so it is built from one of the
/// three constructors below and then narrowed with the chainable setters —
/// `SerializeOptions::default().width(1).strip_comments()` — rather than by
/// struct literal. Every field has a setter here; a future field can be added
/// without a breaking release.
impl SerializeOptions {
    /// Compact single-line output with no insignificant whitespace.
    pub fn compact() -> Self {
        Self {
            pretty: false,
            ..Self::default()
        }
    }

    /// Pretty-printed output with the given number of spaces per indent level.
    pub fn pretty(indent: u8) -> Self {
        Self {
            pretty: true,
            indent,
            ..Self::default()
        }
    }

    /// This style with the given spaces-per-indent-level (see `indent`). The
    /// chainable twin of the [`pretty`](Self::pretty) constructor, for setting
    /// the indent on options you already have.
    pub fn indent(self, indent: u8) -> Self {
        Self { indent, ..self }
    }

    /// This style with `lossless` enabled (see the field). Builder-style so
    /// `SerializeOptions::default().lossless()` reads naturally.
    pub fn lossless(self) -> Self {
        Self {
            lossless: true,
            ..self
        }
    }

    /// This style with comments stripped (see `strip_comments`).
    pub fn strip_comments(self) -> Self {
        Self {
            strip_comments: true,
            ..self
        }
    }

    /// This style with the given inline-vs-expanded column budget for
    /// TOML/YAML/fig (see [`width`](Self::width) for its two limits — `0` is
    /// "unset", and YAML's root is always block). Builder-style, e.g.
    /// `SerializeOptions::default().width(120)`, or `.width(1)` to force block.
    pub fn width(self, width: u16) -> Self {
        Self { width, ..self }
    }
}

impl From<SerializeOptions> for capi::FigSerializeOptions {
    fn from(o: SerializeOptions) -> Self {
        capi::FigSerializeOptions {
            pretty: o.pretty,
            indent: o.indent,
            strip_comments: o.strip_comments,
            lossless: o.lossless,
            width: o.width,
            // Not a public style option — the editors' splice path sets this
            // directly (see `value_text`).
            flow: false,
            splice: false,
        }
    }
}

/// The linked fig library's version, from [`version`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
}

/// The version of the linked fig core, decoded from the packed
/// `(major << 16) | (minor << 8) | patch` that `fig_version` returns.
pub fn version() -> Version {
    let packed = capi::version();
    Version {
        major: (packed >> 16) as u8,
        minor: (packed >> 8) as u8,
        patch: packed as u8,
    }
}

/// The linked fig core's version as a `"major.minor.patch"` string.
pub fn version_string() -> &'static str {
    capi::version_string()
}

/// What this build of the fig core can do with a format: the capabilities its
/// language declared when it was registered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Capabilities {
    /// [`Document::parse`] accepts this format.
    pub read: bool,
    /// The [`Editor`] accepts this format.
    pub edit: bool,
    /// The serializers can write this format.
    pub serialize: bool,
    /// The format has a reference layer — anchors, aliases, `<<` merges,
    /// tags (YAML's). A document leaving such a format for one without is
    /// collapsed first (aliases to copies, merges flattened, tags applied);
    /// one written to a format that has the layer too keeps it.
    pub references: bool,
}

impl Capabilities {
    pub const fn new(read: bool, edit: bool, serialize: bool) -> Self {
        Capabilities {
            read,
            edit,
            serialize,
            references: false,
        }
    }

    /// The same capabilities with `references` set.
    pub const fn with_references(mut self, references: bool) -> Self {
        self.references = references;
        self
    }
}

/// Query what this build can do with `format` (read/edit/serialize). Lets a host
/// pick a working format up front instead of probing via `UnsupportedFormat`.
pub fn capabilities(format: Format) -> Capabilities {
    let bits = capi::format_capabilities(format.to_c());
    Capabilities {
        read: bits & (1 << 0) != 0,
        edit: bits & (1 << 1) != 0,
        serialize: bits & (1 << 2) != 0,
        references: bits & (1 << 3) != 0,
    }
}

#[derive(Debug)]
pub struct Document {
    inner: capi::Document,
}

impl Document {
    pub fn parse(input: &[u8], format: Format) -> Result<Self, Error> {
        // `parse_ex` reports a failure as `fig_parse_ex` fills its `FigError`;
        // on a parse error we project its message/location into
        // `Error::Parse`. Other statuses fold through `from_status` as usual.
        match capi::Document::parse_ex(input, format.to_c()) {
            Ok(inner) => Ok(Self { inner }),
            Err(e) => Err(Error::from_fig_error(&e)),
        }
    }
}

/// Read-path traversal over the parsed node graph: the public `to_value`, plus
/// the lower-level accessors the serde deserializer walks.
impl Document {
    /// Read the whole document into an owned [`Value`] tree — the non-serde
    /// structural read, the mirror of [`Value::serialize`]. An empty document is
    /// [`Value::Null`].
    ///
    /// A YAML alias reads as the value of the node its anchor names, and a
    /// `<<` merge key is flattened into its mapping. An alias with no value —
    /// its anchor undefined, or the alias inside the node it names — is
    /// [`Error::Parse`].
    pub fn to_value(&self) -> Result<Value, Error> {
        let resolved = self.resolve_aliases()?;
        let doc = resolved.as_ref().unwrap_or(self);
        match doc.root() {
            None => Ok(Value::Null),
            Some(id) => doc.node_to_value(id),
        }
    }

    /// The document values are read from: a copy with every alias resolved
    /// and every `<<` merge flattened, or `None` when there is neither and
    /// `self` reads as it is. The node walk below never meets an alias.
    pub(crate) fn resolve_aliases(&self) -> Result<Option<Document>, Error> {
        match self.inner.resolve_aliases() {
            Ok(resolved) => Ok(resolved.map(|inner| Document { inner })),
            Err(e) => Err(Error::from_fig_error(&e)),
        }
    }

    fn node_to_value(&self, id: FigNodeId) -> Result<Value, Error> {
        // A format-specific scalar keeps its kind here; `kind` reads it as
        // the string or int it is written as, for the serde path.
        if let Some((kind, text)) = self.extended(id) {
            return Ok(Value::Extended { kind, text });
        }
        let kind = self.kind(id);
        match kind {
            FigNodeKind::Null => Ok(Value::Null),
            FigNodeKind::Bool => Ok(Value::Bool(self.get_bool(id).ok_or(Error::Internal)?)),
            FigNodeKind::Int | FigNodeKind::Float => {
                let raw = self.number_raw(id).ok_or(Error::Internal)??;
                Value::parse_number(raw, kind == FigNodeKind::Float)
            }
            FigNodeKind::String => Ok(Value::Str(
                self.get_str(id).ok_or(Error::Internal)??.to_owned(),
            )),
            FigNodeKind::Sequence => {
                let mut items = Vec::with_capacity(self.child_count(id));
                let mut next = self.first_child(id);
                while let Some(child) = next {
                    items.push(self.node_to_value(child)?);
                    next = self.next_sibling(child);
                }
                Ok(Value::Seq(items))
            }
            FigNodeKind::Mapping => {
                let mut entries = Vec::with_capacity(self.child_count(id));
                let mut next = self.first_child(id);
                while let Some(kv) = next {
                    let key = self.kv_key(kv).ok_or(Error::Internal)?;
                    let value = match self.kv_value(kv) {
                        Some(vid) => self.node_to_value(vid)?,
                        None => Value::Null,
                    };
                    entries.push((self.node_to_value(key)?, value));
                    next = self.next_sibling(kv);
                }
                Ok(Value::Map(entries))
            }
            // A bare keyvalue or an invalid id can't stand alone as a value,
            // and `to_value` reads a document whose aliases are resolved.
            FigNodeKind::KeyValue
            | FigNodeKind::Invalid
            | FigNodeKind::Alias
            | FigNodeKind::Extended => Err(Error::Internal),
        }
    }

    /// The root node, or `None` for an empty document.
    pub(crate) fn root(&self) -> Option<FigNodeId> {
        normalize(self.inner.root())
    }

    /// The node's kind as the serde path reads it: an extended scalar is
    /// the string it is written as, or the int a char literal's codepoint
    /// is, so a struct field of either type takes one. `node_to_value`
    /// asks `extended` first and keeps the kind.
    pub(crate) fn kind(&self, node: FigNodeId) -> FigNodeKind {
        match self.inner.node_kind(node) {
            FigNodeKind::Extended => match self.extended(node) {
                Some((ExtKind::CharLiteral, _)) => FigNodeKind::Int,
                _ => FigNodeKind::String,
            },
            k => k,
        }
    }

    pub(crate) fn first_child(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(self.inner.first_child(node))
    }

    pub(crate) fn next_sibling(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(self.inner.next_sibling(node))
    }

    pub(crate) fn child_count(&self, node: FigNodeId) -> usize {
        self.inner.child_count(node)
    }

    pub(crate) fn kv_key(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(self.inner.keyvalue_key(node))
    }

    pub(crate) fn kv_value(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(self.inner.keyvalue_value(node))
    }

    pub(crate) fn get_bool(&self, node: FigNodeId) -> Option<bool> {
        self.inner.node_bool(node)
    }

    /// The raw source text of a numeric scalar. Borrows document memory.
    pub(crate) fn number_raw(&self, node: FigNodeId) -> Option<Result<&str, Error>> {
        self.inner
            .node_number(node)
            .map(|bytes| std::str::from_utf8(bytes).map_err(|_| Error::Utf8))
    }

    /// The bytes of a string scalar, as UTF-8. Borrows document memory.
    pub(crate) fn get_str(&self, node: FigNodeId) -> Option<Result<&str, Error>> {
        self.inner
            .node_string(node)
            .map(|bytes| std::str::from_utf8(bytes).map_err(|_| Error::Utf8))
    }

    /// If `node` is a format-specific extended scalar (TOML datetime, ZON
    /// enum/char literal), recover its [`ExtKind`] and text; `None` otherwise.
    /// Used only by the structural [`Document::to_value`] read — the serde path
    /// reads these as plain strings/ints.
    pub(crate) fn extended(&self, node: FigNodeId) -> Option<(ExtKind, String)> {
        let (kind, bytes) = self.inner.node_extended(node)?;
        let ext = ExtKind::from_c(c_int::from(kind.ordinal()))?;
        let text = std::str::from_utf8(bytes).ok()?.to_owned();
        Some((ext, text))
    }
}

/// Cross-format conversion + serialization diagnostics over the parsed document.
impl Document {
    /// Render the whole document to `format` with the default output style — the
    /// cross-format conversion primitive (e.g. parse YAML, emit JSON). Unlike
    /// `to_value().serialize()`, this preserves comments carried on the source
    /// where the target allows, and collapses YAML's reference layer when leaving
    /// YAML. A value the target cannot represent is an [`Error::UnsupportedFormat`]
    /// unless `lossless` is set (see [`Document::serialize_with`]).
    pub fn serialize(&self, format: Format) -> Result<String, Error> {
        self.serialize_with(format, SerializeOptions::default())
    }

    /// As [`Document::serialize`], with `options` controlling output style,
    /// comment stripping, and lossless `$fig`-envelope round-tripping.
    pub fn serialize_with(
        &self,
        format: Format,
        options: SerializeOptions,
    ) -> Result<String, Error> {
        let core_options: capi::FigSerializeOptions = options.into();
        let bytes = Error::check(self.inner.serialize_to_vec(format.to_c(), Some(&core_options)))?;
        String::from_utf8(bytes).map_err(|_| Error::Utf8)
    }

    /// Report what serializing the whole document to `format` would silently lose
    /// (comments dropped/degraded, values dropped/degraded), using the same
    /// pipeline [`Document::serialize_with`] prints from. Returns one [`Warning`]
    /// per lossy event (empty if nothing is lost).
    pub fn diagnose(
        &self,
        format: Format,
        options: SerializeOptions,
    ) -> Result<Vec<Warning>, Error> {
        let core_options: capi::FigSerializeOptions = options.into();
        let warnings = Error::check(self.inner.diagnose_to_vec(format.to_c(), Some(&core_options)))?;
        Ok(warnings.iter().map(Warning::from_core).collect())
    }
}

fn normalize(id: FigNodeId) -> Option<FigNodeId> {
    if id == FIG_NODE_NONE { None } else { Some(id) }
}

#[cfg(test)]
mod tests {
    use super::{Document, Error};
    use crate::test_languages::by_name;

    #[test]
    fn parses_json_document() {
        let doc = Document::parse(br#"{"name":"fig","ok":true}"#, by_name("json"));
        assert!(doc.is_ok());
    }

    #[test]
    fn parse_error_is_reported() {
        let err = Document::parse(br#"{"name":"fig""#, by_name("json")).unwrap_err();
        let Error::Parse(detail) = &err else {
            panic!("expected Error::Parse, got {err:?}");
        };
        // The language says why and where: past the end of the input.
        assert!(!detail.message.is_empty());
        assert_eq!(detail.byte_offset, Some(13));
    }

    #[test]
    fn version_and_capabilities() {
        use super::{capabilities, version, version_string};
        let v = version();
        // The packed version round-trips through the string form.
        assert_eq!(
            version_string(),
            format!("{}.{}.{}", v.major, v.minor, v.patch)
        );
        // JSON is always fully supported in any build.
        let json = capabilities(by_name("json"));
        assert!(json.read && json.edit && json.serialize);
    }

    #[test]
    fn document_serialize_converts_cross_format() {
        // YAML in, JSON out — the conversion primitive, not a value rebuild.
        let doc = Document::parse(b"name: fig\nnums:\n- 1\n- 2\n", by_name("yaml")).unwrap();
        assert_eq!(
            doc.serialize(by_name("json")).unwrap(),
            "{\n  \"name\": \"fig\",\n  \"nums\": [\n    1,\n    2\n  ]\n}\n",
        );
    }

    #[test]
    fn document_diagnose_reports_dropped_null() {
        use super::{SerializeOptions, WarningCause, WarningCode};
        let doc = Document::parse(b"a: null\nb: 1\n", by_name("yaml")).unwrap();
        // A null can't survive in TOML → one value-dropped warning at path "a".
        let warns = doc
            .diagnose(by_name("toml"), SerializeOptions::default())
            .unwrap();
        assert_eq!(warns.len(), 1);
        assert_eq!(warns[0].code, WarningCode::ValueDropped);
        assert_eq!(warns[0].cause, WarningCause::FormatLimitation);
        assert_eq!(warns[0].path, "a");
        // Lossless preserves the null → nothing lost.
        let none = doc
            .diagnose(by_name("toml"), SerializeOptions::default().lossless())
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn parse_error_message_is_surfaced_in_display() {
        let err = Document::parse(br#"{"name":"fig""#, by_name("json")).unwrap_err();
        // Display includes the core's message after the generic prefix.
        assert!(err.to_string().starts_with("failed to parse input: "));
    }

    #[test]
    fn editor_comment_ops_add_set_and_delete() {
        use super::{Editor, Segment};
        let mut ed = Editor::open(b"a: 1\nb: 2\n", by_name("yaml")).unwrap();
        ed.add_leading_comment(&[Segment::Key("b")], "why").unwrap();
        ed.set_trailing_comment(&[Segment::Key("b")], "two")
            .unwrap();
        assert_eq!(ed.source().unwrap(), "a: 1\n# why\nb: 2 # two\n");
        ed.delete_trailing_comment(&[Segment::Key("b")]).unwrap();
        ed.delete_leading_comment(&[Segment::Key("b")]).unwrap();
        assert_eq!(ed.source().unwrap(), "a: 1\nb: 2\n");
    }

    #[test]
    fn editor_comments_unsupported_in_strict_json() {
        use super::{Editor, Error, Segment};
        let mut ed = Editor::open(br#"{"a":1}"#, by_name("json")).unwrap();
        assert!(matches!(
            ed.add_leading_comment(&[Segment::Key("a")], "x"),
            Err(Error::UnsupportedFormat)
        ));
    }
}
