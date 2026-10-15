// Lets the `derive`-generated code refer to this crate as `fig::…` even from
// within the crate's own tests and examples.
extern crate self as fig;

mod diagnostics;
mod editor;
mod error;
mod value;

// The raw FFI layer moved to the `fig-sys` crate. Alias it as `ffi` so every
// `ffi::…` / `crate::ffi::…` reference in this crate keeps resolving unchanged,
// and so `fig-sys`'s build script (via its `links = "fig"`) links `libfig.a`.
pub(crate) use fig_sys as ffi;

#[cfg(feature = "derive")]
mod convert;
#[cfg(feature = "serde")]
mod de;
#[cfg(feature = "serde")]
mod ser;

use std::os::raw::c_int;
use std::ptr::NonNull;

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

use ffi::{FIG_NODE_NONE, FigNodeId, FigNodeKind};

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
/// registered with the linked fig core.
///
/// A `Format` is obtained by name — [`Format::by_name`]`("yaml")` — or from
/// [`language::register`] and [`Format::detect`]; it cannot be built from an
/// integer. Handles are assigned in registration
/// order and differ between processes: persist [`Format::name`] and resolve it
/// again with [`Format::by_name`], never the handle.
///
/// The native library carries no language: each is a crate of its own
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
        if value <= 0 || unsafe { ffi::fig_format_name(value) }.is_null() {
            return None;
        }
        Some(Format(value))
    }

    /// The registered format named `name` (`"json5"`, `"yaml"`, `"yaml-1.1"`,
    /// or a language registered at runtime). `None` when nothing answers to it
    /// in this process — a misspelling, or a language this build compiled out.
    pub fn by_name(name: &str) -> Option<Format> {
        let c = std::ffi::CString::new(name).ok()?;
        let value = unsafe { ffi::fig_format_by_name(c.as_ptr()) };
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
        let ptr = unsafe { ffi::fig_format_name(self.0) };
        if ptr.is_null() {
            return "";
        }
        // Safety: a non-NULL name is NUL-terminated and lives as long as the
        // process (the registry is append-only).
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_str()
            .unwrap_or("")
    }

    /// The documents of a multi-document stream in this format (YAML's
    /// `---`/`...`), each a byte range of `input` to parse as a document of
    /// its own. An empty stream is one document.
    /// [`Error::UnsupportedOperation`] for a format without streams.
    pub fn split_stream(self, input: &[u8]) -> Result<Vec<language::StreamDocument>, Error> {
        let mut docs: *mut ffi::FigStreamDoc = std::ptr::null_mut();
        let mut count = 0usize;
        let mut err = ffi::FigError::new();
        let status = unsafe {
            ffi::fig_stream_split(
                input.as_ptr(),
                input.len(),
                self.to_c(),
                &mut docs,
                &mut count,
                &mut err,
            )
        };
        if status == ffi::FigStatus::PARSE_ERROR {
            return Err(Error::Parse(crate::error::ParseError::from_ffi(&err)));
        }
        Error::from_status(status)?;
        let out = (0..count)
            .map(|i| {
                let d = unsafe { *docs.add(i) };
                language::StreamDocument {
                    start: d.start,
                    end: d.end,
                    explicit: d.marked,
                }
            })
            .collect();
        unsafe { ffi::fig_stream_free(docs, count) };
        Ok(out)
    }

    /// The first format `order` names whose parser accepts `input`, tried in
    /// that order: content sniffing, with the order the caller's to choose.
    /// The grammars overlap, so put the strictest first. A name nothing
    /// answers to is skipped.
    pub fn detect(input: &[u8], order: &[&str]) -> Option<Format> {
        let names: Vec<std::ffi::CString> = order
            .iter()
            .filter_map(|n| std::ffi::CString::new(*n).ok())
            .collect();
        let ptrs: Vec<*const std::os::raw::c_char> = names.iter().map(|n| n.as_ptr()).collect();
        let mut out: std::os::raw::c_int = -1;
        let status = unsafe {
            ffi::fig_detect_order(
                input.as_ptr(),
                input.len(),
                ptrs.as_ptr(),
                ptrs.len(),
                &mut out,
            )
        };
        if status != ffi::FigStatus::OK {
            return None;
        }
        Format::from_c(out)
    }
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

impl From<SerializeOptions> for ffi::FigSerializeOptions {
    fn from(o: SerializeOptions) -> Self {
        ffi::FigSerializeOptions {
            size: std::mem::size_of::<ffi::FigSerializeOptions>() as u32,
            pretty: u8::from(o.pretty),
            indent: o.indent,
            strip_comments: u8::from(o.strip_comments),
            lossless: u8::from(o.lossless),
            width: o.width,
            // Not a public style option — the editors' splice path sets this
            // directly (see `value_text`).
            flow: 0,
            splice: 0,
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
    let packed = unsafe { ffi::fig_version() };
    Version {
        major: (packed >> 16) as u8,
        minor: (packed >> 8) as u8,
        patch: packed as u8,
    }
}

/// The linked fig core's version as a `"major.minor.patch"` string.
pub fn version_string() -> &'static str {
    // Safety: `fig_version_string` returns a static NUL-terminated ASCII string
    // owned by the library, valid for the whole program.
    let ptr = unsafe { ffi::fig_version_string() };
    unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_str()
        .unwrap_or("")
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
    let bits = unsafe { ffi::fig_format_capabilities(format.to_c()) };
    Capabilities {
        read: bits & (1 << 0) != 0,
        edit: bits & (1 << 1) != 0,
        serialize: bits & (1 << 2) != 0,
        references: bits & (1 << 3) != 0,
    }
}

#[derive(Debug)]
pub struct Document {
    raw: NonNull<ffi::FigDocument>,
}

// SAFETY: a `FigDocument` is owned by this handle alone and tied to no
// thread: it is allocated from the process allocator (libc's, or a
// thread-safe one where there is none; `activeAllocator` in c_api.zig),
// holds no thread-local state, and reads languages from a registry built for
// concurrent use, whose Rust languages are `Send + Sync`. So it may be moved
// to, used on and freed on another thread. Not `Sync`: nothing promises two
// threads may read one document at once.
unsafe impl Send for Document {}

impl Document {
    pub fn parse(input: &[u8], format: Format) -> Result<Self, Error> {
        let mut raw = std::ptr::null_mut();

        // `fig_parse_ex` fills `err` on failure; on a parse error we project its
        // message/location into `Error::Parse`. Other statuses fold through
        // `from_status` as usual (the struct is meaningful only on failure).
        let mut err = ffi::FigError::new();
        let status = unsafe {
            ffi::fig_parse_ex(
                input.as_ptr(),
                input.len(),
                format.to_c(),
                &mut raw,
                &mut err,
            )
        };
        if status != ffi::FigStatus::OK {
            if status == ffi::FigStatus::PARSE_ERROR {
                return Err(Error::Parse(crate::error::ParseError::from_ffi(&err)));
            }
            Error::from_status(status)?;
        }

        let raw = NonNull::new(raw).ok_or(Error::Internal)?;
        Ok(Self { raw })
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
        let mut raw = std::ptr::null_mut();
        let mut err = ffi::FigError::new();
        let status = unsafe { ffi::fig_document_resolve_aliases(self.ptr(), &mut raw, &mut err) };
        if status == ffi::FigStatus::PARSE_ERROR {
            return Err(Error::Parse(crate::error::ParseError::from_ffi(&err)));
        }
        Error::from_status(status)?;
        Ok(NonNull::new(raw).map(|raw| Document { raw }))
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
            FigNodeKind::Keyvalue
            | FigNodeKind::Invalid
            | FigNodeKind::Alias
            | FigNodeKind::Extended => Err(Error::Internal),
        }
    }

    fn ptr(&self) -> *const ffi::FigDocument {
        self.raw.as_ptr()
    }

    /// The root node, or `None` for an empty document.
    pub(crate) fn root(&self) -> Option<FigNodeId> {
        normalize(unsafe { ffi::fig_document_root(self.ptr()) })
    }

    /// The node's kind as the serde path reads it: an extended scalar is
    /// the string it is written as, or the int a char literal's codepoint
    /// is, so a struct field of either type takes one. `node_to_value`
    /// asks `extended` first and keeps the kind.
    pub(crate) fn kind(&self, node: FigNodeId) -> FigNodeKind {
        // `from_c` is the sole gate that turns the raw ABI int into the enum,
        // mapping any unknown/future kind to `Invalid` instead of risking UB.
        match FigNodeKind::from_c(unsafe { ffi::fig_node_kind(self.ptr(), node) }) {
            FigNodeKind::Extended => match self.extended(node) {
                Some((ExtKind::CharLiteral, _)) => FigNodeKind::Int,
                _ => FigNodeKind::String,
            },
            k => k,
        }
    }

    pub(crate) fn first_child(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(unsafe { ffi::fig_node_first_child(self.ptr(), node) })
    }

    pub(crate) fn next_sibling(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(unsafe { ffi::fig_node_next_sibling(self.ptr(), node) })
    }

    pub(crate) fn child_count(&self, node: FigNodeId) -> usize {
        unsafe { ffi::fig_node_child_count(self.ptr(), node) }
    }

    pub(crate) fn kv_key(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(unsafe { ffi::fig_keyvalue_key(self.ptr(), node) })
    }

    pub(crate) fn kv_value(&self, node: FigNodeId) -> Option<FigNodeId> {
        normalize(unsafe { ffi::fig_keyvalue_value(self.ptr(), node) })
    }

    pub(crate) fn get_bool(&self, node: FigNodeId) -> Option<bool> {
        let mut out = false;
        unsafe { ffi::fig_node_bool(self.ptr(), node, &mut out) }.then_some(out)
    }

    /// The raw source text of a numeric scalar. Borrows document memory.
    pub(crate) fn number_raw(&self, node: FigNodeId) -> Option<Result<&str, Error>> {
        self.scalar_bytes(node, ffi::fig_node_number)
            .map(|bytes| std::str::from_utf8(bytes).map_err(|_| Error::Utf8))
    }

    /// The bytes of a string scalar, as UTF-8. Borrows document memory.
    pub(crate) fn get_str(&self, node: FigNodeId) -> Option<Result<&str, Error>> {
        self.scalar_bytes(node, ffi::fig_node_string)
            .map(|bytes| std::str::from_utf8(bytes).map_err(|_| Error::Utf8))
    }

    /// If `node` is a format-specific extended scalar (TOML datetime, ZON
    /// enum/char literal), recover its [`ExtKind`] and text; `None` otherwise.
    /// Used only by the structural [`Document::to_value`] read — the serde path
    /// reads these as plain strings/ints.
    pub(crate) fn extended(&self, node: FigNodeId) -> Option<(ExtKind, String)> {
        let mut kind: c_int = 0;
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        let ok = unsafe { ffi::fig_node_extended(self.ptr(), node, &mut kind, &mut ptr, &mut len) };
        if !ok {
            return None;
        }
        let ext = ExtKind::from_c(kind)?;
        let text = if len == 0 {
            String::new()
        } else {
            // Safety: on success the ABI guarantees `ptr` points to `len` bytes
            // owned by the document, valid until our `Drop`.
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
            std::str::from_utf8(bytes).ok()?.to_owned()
        };
        Some((ext, text))
    }

    /// Shared helper for the byte-returning scalar accessors. The returned
    /// slice borrows memory owned by the document (valid until drop), which we
    /// tie to `&self`.
    fn scalar_bytes(
        &self,
        node: FigNodeId,
        accessor: unsafe extern "C" fn(
            *const ffi::FigDocument,
            FigNodeId,
            *mut *const u8,
            *mut usize,
        ) -> bool,
    ) -> Option<&[u8]> {
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        let ok = unsafe { accessor(self.ptr(), node, &mut ptr, &mut len) };
        if !ok {
            return None;
        }
        if len == 0 {
            return Some(&[]);
        }
        // Safety: on success the ABI guarantees `ptr` points to `len` bytes
        // owned by the document, valid until `fig_document_destroy` (i.e. our
        // `Drop`). The returned borrow is bounded by `&self`.
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
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
        let ffi_options: ffi::FigSerializeOptions = options.into();
        let mut ptr_out: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        Error::from_status(unsafe {
            ffi::fig_document_serialize(
                self.raw.as_ptr(),
                format.to_c(),
                &ffi_options,
                &mut ptr_out,
                &mut len,
            )
        })?;
        // Safety: on success the ABI guarantees `len` bytes at `ptr_out`, owned by
        // the handle and valid until the next serialize/destroy. Copy out now.
        let bytes = if len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(ptr_out, len) }
        };
        Ok(std::str::from_utf8(bytes)
            .map_err(|_| Error::Utf8)?
            .to_owned())
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
        let ffi_options: ffi::FigSerializeOptions = options.into();
        let mut count: usize = 0;
        Error::from_status(unsafe {
            ffi::fig_document_diagnose(self.raw.as_ptr(), format.to_c(), &ffi_options, &mut count)
        })?;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let mut w = ffi::FigWarning::new();
            Error::from_status(unsafe { ffi::fig_document_warning(self.raw.as_ptr(), i, &mut w) })?;
            // Safety: on `OK`, `w` is filled with a warning whose path/note point
            // into the handle's arena, valid until the next diagnose/destroy.
            out.push(unsafe { Warning::from_ffi(&w) });
        }
        Ok(out)
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe {
            ffi::fig_document_destroy(self.raw.as_ptr());
        }
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
