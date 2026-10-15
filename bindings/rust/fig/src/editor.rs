//! Comment-preserving, in-place editing of a document in any editable format.
//!
//! Unlike [`crate::Value::serialize`], which reserializes a whole value,
//! [`Editor`] splices only the bytes of the node you change — comments, key
//! order, blank lines, and quoting style everywhere else are left byte-
//! identical. Inserted values are rendered by fig's serializer (via [`Value`]),
//! then spliced in; the core editor re-frames indentation and flow/block context
//! at the site.
//!
//! # Naming
//!
//! [`Editor`]'s edit methods follow one naming scheme.
//!
//! * **Value edits** are `<verb>_value` — `replace_value`, `insert_value`,
//!   `set_value`, `append_value`, `prepend_value` — each taking any
//!   `impl Into<Value>`, and each with a `<verb>_value_with` twin taking a
//!   [`SerializeOptions`] for the spliced value's own rendering. The one
//!   value edit at an index is `insert_item` (and `insert_item_with`),
//!   named by its noun as `delete_item` is, since `insert_value` inserts a
//!   key. A typed
//!   struct goes in through `to_value()` (the `derive` feature) or
//!   `fig::to_value(&x)?` (the `serde` feature).
//! * **Structural edits** are `<verb>_<noun>`, where the noun is what the path
//!   names: a `key` (a mapping entry), an `item` (a sequence element, by
//!   index), or a `container` (a scattered TOML/INI/fig section). The verbs
//!   are `delete`, `move`, `reorder` and `rename`: `delete_key`,
//!   `delete_item`, `delete_container`; `move_key`, `move_item`,
//!   `move_container`; `reorder_keys`, `reorder_items`, `reorder_containers`;
//!   `rename_key`, `rename_container`.
//! * **Comments** are named by their anchor — `leading`, `trailing`,
//!   `dangling` — and a read (`leading_comment`) returns the whole comment at
//!   that anchor, which `delete_<anchor>_comment` removes. The write verb says
//!   what the anchor holds: `add_` for the two that hold a run of lines
//!   (`add_leading_comment`, `add_dangling_comment` append to it), `set_` for
//!   the one that holds a single line (`set_trailing_comment` replaces it).

use fig_core::kit::ast::PathSegment;

use crate::capi::{self, Status};
use crate::error::{EditTarget, Error, edit_status};
use crate::value::{Value, value_text, value_text_with};
use crate::{Format, SerializeOptions};

/// One step of a path into a document: a mapping key or a sequence index.
#[derive(Clone, Copy, Debug)]
pub enum Segment<'a> {
    Key(&'a str),
    Index(usize),
}

impl<'a> From<&'a str> for Segment<'a> {
    fn from(key: &'a str) -> Self {
        Segment::Key(key)
    }
}

impl From<usize> for Segment<'_> {
    fn from(index: usize) -> Self {
        Segment::Index(index)
    }
}

/// The core's path. The returned segments borrow the key bytes of `path`.
pub(crate) fn to_core_path<'a>(path: &[Segment<'a>]) -> Vec<PathSegment<'a>> {
    path.iter()
        .map(|seg| match seg {
            Segment::Key(k) => PathSegment::Key(k.as_bytes()),
            Segment::Index(i) => PathSegment::Index(*i),
        })
        .collect()
}

/// The core's key list. The returned entries borrow the key bytes of `keys`.
pub(crate) fn to_core_keys<S: AsRef<str>>(keys: &[S]) -> Vec<&[u8]> {
    keys.iter().map(|k| k.as_ref().as_bytes()).collect()
}

/// An in-place editor over an owned copy of a document's source.
#[derive(Debug)]
pub struct Editor {
    inner: capi::Editor,
    /// The document's format, used to render replacement/insertion splice text in
    /// the same dialect (e.g. a `Value::Str` becomes `"x"` for TOML/JSON but a
    /// bare `x` for YAML). Hardcoding one format would splice syntactically wrong
    /// text for the others and fail the editor's reparse.
    format: Format,
}

impl Editor {
    /// Open an editor over a copy of `input` in the given format.
    pub fn open(input: &[u8], format: Format) -> Result<Self, Error> {
        match capi::Editor::create(input, format.to_c()) {
            Ok(inner) => Ok(Self { inner, format }),
            // `Editor::create` has no `_ex` twin, and its parse is the one
            // `parse_ex` runs: ask that for the core's diagnostic, so the
            // error names what failed rather than only that something did.
            Err(Status::ParseError) => Err(match crate::Document::parse(input, format) {
                Err(e @ Error::Parse(_)) => e,
                _ => Error::Parse(crate::error::ParseError::generic()),
            }),
            Err(status) => Err(Error::from_status(status).err().unwrap_or(Error::Internal)),
        }
    }

    // ── value edits (over `Value`) ──────────────────────────────────────────

    /// Replace the value at `path` with `value` — any `impl Into<Value>`: a
    /// scalar (`9i64`, `"x"`, `true`), a built [`Value`], or a `&Value`.
    pub fn replace_value(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
    ) -> Result<(), Error> {
        let value = value.into();
        let repl = value_text(&value, self.format)?;
        self.replace_rendered(path, &repl, &value)
    }

    /// Rename the key at `path` to `key`, a name the format spells as it
    /// spells a key (`.k` in ZON, `"k"` in JSON, `<key>k</key>` in plist).
    ///
    /// A path that names no key (one ending in an index, or the empty path)
    /// and a `key` another entry of the mapping already holds are
    /// [`Error::InvalidArgument`], and nothing is written.
    pub fn rename_key(&mut self, path: &[Segment], key: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.replace_named_key(&p, key.as_bytes());
        edit_status(status, self.format, EditTarget::Key)
    }

    /// Insert `key: value` into the mapping at `path` (empty path = root).
    /// `value` is any `impl Into<Value>`.
    pub fn insert_value(
        &mut self,
        path: &[Segment],
        key: &str,
        value: impl Into<Value>,
    ) -> Result<(), Error> {
        // The key is a NAME; the editor spells it as the format spells a
        // key (`.k` in ZON, `"k"` in JSON) — a string value, which this used
        // to send, is not a key in ZON.
        let val = value_text(&value.into(), self.format)?;
        let p = to_core_path(path);
        let status = self
            .inner
            .insert_named_key(&p, key.as_bytes(), val.as_bytes());
        edit_status(status, self.format, EditTarget::Value)
    }

    /// Upsert a mapping value: replace the value at `path`, or insert it when
    /// only the trailing key is absent. Folds the common
    /// `replace_value` → (on [`Error::NotFound`]) `insert_value` two-step into a
    /// single call. `path` must end in a key (it only ever creates a mapping
    /// entry).
    ///
    /// Missing intermediate containers are created for you (`mkdir -p` for
    /// config). For YAML they are created as **block** mappings, so a fresh
    /// nested path reads like hand-written YAML and accepts any value:
    ///
    /// ```text
    /// set_value(a.b.c, 1)  ->  a:
    ///                            b:
    ///                              c: 1
    /// ```
    ///
    /// The dotted-key formats (TOML, fig) instead seed the idiomatic flow chain
    /// (`a = { b = 1 }`), which `fig fmt` canonicalizes to `a.b = 1`.
    ///
    /// Two things to know:
    ///
    /// * A **flow** container cannot hold a block-spelled value. Setting a map or
    ///   sequence *into a flow container the document already has*
    ///   (`a: {b: 1}`, `a: {}`) fails with [`Error::InvalidArgument`] rather than
    ///   writing something that no longer means what was passed in. Expand the
    ///   container to block form first, or pass a value that renders in flow.
    /// * A failed `set_value` is a no-op: any container created on the way is
    ///   rolled back, so `Err` always means the document is unchanged.
    pub fn set_value(&mut self, path: &[Segment], value: impl Into<Value>) -> Result<(), Error> {
        let value = value.into();
        let val = value_text(&value, self.format)?;
        self.set_rendered(path, &val, &value)
    }

    // ── value edits with a layout knob (block-vs-inline containers) ─────────
    //
    // The plain `replace_value`/`insert_value`/`set_value` above render a fig
    // container value as inline flow (`k = { … }`) — the only spelling a bare
    // inline splice can carry. These `*_with` twins instead honor `options`
    // (notably `width`): a map/sequence that does not fit renders as a BLOCK
    // section, which the core editor re-frames under the target key
    // (`key` header + `> …` body). This is the width knob for splices — e.g. a
    // short map you want to land one-record-per-line rather than frozen inline.
    // For non-fig formats they behave like the plain methods, plus `options`.

    /// Replace the value at `path`, rendering `value` with `options` (see the
    /// `*_with` note above — a block map/sequence lands as a nested section).
    pub fn replace_value_with(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
        options: SerializeOptions,
    ) -> Result<(), Error> {
        let value = value.into();
        let repl = value_text_with(&value, self.format, options)?;
        self.replace_rendered(path, &repl, &value)
    }

    /// Insert `key: value` into the mapping at `path`, rendering `value` with
    /// `options` (a block map/sequence lands as a nested section).
    pub fn insert_value_with(
        &mut self,
        path: &[Segment],
        key: &str,
        value: impl Into<Value>,
        options: SerializeOptions,
    ) -> Result<(), Error> {
        let val = value_text_with(&value.into(), self.format, options)?;
        let p = to_core_path(path);
        let status = self
            .inner
            .insert_named_key(&p, key.as_bytes(), val.as_bytes());
        edit_status(status, self.format, EditTarget::Value)
    }

    /// Upsert the value at `path`, rendering `value` with `options` (a block
    /// map/sequence lands as a nested section). The width-aware twin of
    /// [`set_value`](Self::set_value).
    pub fn set_value_with(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
        options: SerializeOptions,
    ) -> Result<(), Error> {
        let value = value.into();
        let val = value_text_with(&value, self.format, options)?;
        self.set_rendered(path, &val, &value)
    }

    // A string fig rendered goes to the `_string` exports with the string
    // beside its spelling, so the string it replaces keeps its quote style
    // (a YAML `"debug"` set to `info` stays `"info"`); anything else is
    // spliced as rendered.

    fn replace_rendered(
        &mut self,
        path: &[Segment],
        repl: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = match value {
            Value::Str(s) => self.inner.replace_string(&p, repl.as_bytes(), s.as_bytes()),
            _ => self.inner.replace_val(&p, repl.as_bytes()),
        };
        edit_status(status, self.format, EditTarget::Value)
    }

    fn set_rendered(&mut self, path: &[Segment], val: &str, value: &Value) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = match value {
            Value::Str(s) => self.inner.set_string(&p, val.as_bytes(), s.as_bytes()),
            _ => self.inner.set(&p, val.as_bytes()),
        };
        edit_status(status, self.format, EditTarget::Value)
    }

    /// Append `value` (any `impl Into<Value>`) to the sequence at `path`.
    pub fn append_value(&mut self, path: &[Segment], value: impl Into<Value>) -> Result<(), Error> {
        let val = value_text(&value.into(), self.format)?;
        self.append_text(path, &val)
    }

    /// Append `value` to the sequence at `path`, rendering it with `options`.
    /// The width-aware twin of [`append_value`](Self::append_value).
    pub fn append_value_with(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
        options: SerializeOptions,
    ) -> Result<(), Error> {
        let val = value_text_with(&value.into(), self.format, options)?;
        self.append_text(path, &val)
    }

    fn append_text(&mut self, path: &[Segment], val: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.append_seq(&p, val.as_bytes());
        edit_status(status, self.format, EditTarget::Value)
    }

    /// Prepend `value` (any `impl Into<Value>`) to the sequence at `path`.
    pub fn prepend_value(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
    ) -> Result<(), Error> {
        let val = value_text(&value.into(), self.format)?;
        self.prepend_text(path, &val)
    }

    /// Prepend `value` to the sequence at `path`, rendering it with `options`.
    /// The width-aware twin of [`prepend_value`](Self::prepend_value).
    pub fn prepend_value_with(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
        options: SerializeOptions,
    ) -> Result<(), Error> {
        let val = value_text_with(&value.into(), self.format, options)?;
        self.prepend_text(path, &val)
    }

    fn prepend_text(&mut self, path: &[Segment], val: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.prepend_seq(&p, val.as_bytes());
        edit_status(status, self.format, EditTarget::Value)
    }

    /// Insert `value` (any `impl Into<Value>`) into the sequence at `path` so
    /// it is the item at `index` afterwards: before the item there now, or
    /// after the last when `index` is the length (or `usize::MAX`, the end).
    /// 0 is [`prepend_value`](Self::prepend_value) and the length is
    /// [`append_value`](Self::append_value). A flow item takes a separator
    /// laid out as its sequence is; a block item goes on its own line above
    /// the item at `index` and that item's comment block. An index past the
    /// length is [`Error::NotFound`], and nothing is written.
    pub fn insert_item(
        &mut self,
        path: &[Segment],
        index: usize,
        value: impl Into<Value>,
    ) -> Result<(), Error> {
        let val = value_text(&value.into(), self.format)?;
        self.insert_item_text(path, index, &val)
    }

    /// Insert `value` at `index` in the sequence at `path`, rendering it with
    /// `options`. The width-aware twin of [`insert_item`](Self::insert_item).
    pub fn insert_item_with(
        &mut self,
        path: &[Segment],
        index: usize,
        value: impl Into<Value>,
        options: SerializeOptions,
    ) -> Result<(), Error> {
        let val = value_text_with(&value.into(), self.format, options)?;
        self.insert_item_text(path, index, &val)
    }

    fn insert_item_text(&mut self, path: &[Segment], index: usize, val: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.insert_seq_item(&p, index, val.as_bytes());
        edit_status(status, self.format, EditTarget::Value)
    }

    // ── comment editing ─────────────────────────────────────────────────────

    /// Add an own-line comment ABOVE the node at `path` (the key's line for a
    /// mapping entry), at its indentation, nearest the node. `text` may be
    /// multi-line (one comment line per row). The marker (`#` for YAML, `//` for
    /// JSONC/JSON5) is added for you; strict JSON returns
    /// [`Error::UnsupportedFormat`].
    ///
    /// An element or entry of a one-line flow collection (`members = ["a",
    /// "b"]`, `nested: {k: v}`) shares its parent's line and owns no line to
    /// comment: those paths return [`Error::InvalidArgument`]. Comment the
    /// collection itself, or break it onto one element per line.
    pub fn add_leading_comment(&mut self, path: &[Segment], text: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.add_leading_comment(&p, text.as_bytes());
        edit_status(status, self.format, EditTarget::Comment)
    }

    /// Set the same-line trailing comment on the value at `path`, replacing an
    /// existing one or appending if absent. `text` must be a single line
    /// ([`Error::InvalidArgument`] otherwise); strict JSON returns
    /// [`Error::UnsupportedFormat`]. An element or entry of a one-line flow
    /// collection also returns [`Error::InvalidArgument`]: the end of that line
    /// is the parent's, not the element's.
    pub fn set_trailing_comment(&mut self, path: &[Segment], text: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.set_trailing_comment(&p, text.as_bytes());
        edit_status(status, self.format, EditTarget::Comment)
    }

    /// Remove the own-line comment block immediately above the node at `path`
    /// — every line of it, which is what [`leading_comment`](Self::leading_comment)
    /// reads. A no-op (still `Ok`) when there is none — including an element
    /// or entry of a one-line flow collection, whose block above is its
    /// parent's.
    pub fn delete_leading_comment(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.delete_leading_comments(&p);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Remove the same-line trailing comment on the value at `path`. A no-op
    /// (still `Ok`) when there is none — including an element or entry of a
    /// one-line flow collection, whose line-end comment is its parent's.
    pub fn delete_trailing_comment(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.delete_trailing_comment(&p);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Read the own-line comment block immediately above the node at `path`
    /// (lines joined by `\n`, markers and indentation stripped). `None` when
    /// there is no such block; `Some("")` for a present-but-empty bare marker.
    /// Strict JSON returns [`Error::UnsupportedFormat`].
    ///
    /// `None` too for an element or entry of a one-line flow collection: the
    /// block above `members = ["a", "b"]` belongs to `members`, not to its
    /// items.
    pub fn leading_comment(&self, path: &[Segment]) -> Result<Option<String>, Error> {
        self.read_comment(path, false)
    }

    /// Read the same-line trailing comment on the value at `path` (marker
    /// stripped). `None` when there is none — including an element or entry of
    /// a one-line flow collection, whose line ends with its parent's comment;
    /// `Some("")` for a bare marker. Strict JSON returns
    /// [`Error::UnsupportedFormat`].
    pub fn trailing_comment(&self, path: &[Segment]) -> Result<Option<String>, Error> {
        self.read_comment(path, true)
    }

    /// Shared body for the two comment reads. Returns an owned `String` (copied
    /// out of the handle's reused scratch buffer) so the result stays valid
    /// across later reads, and maps `NOT_FOUND` to `Ok(None)` — distinguishing an
    /// absent comment from a present-but-empty one (`Some(String::new())`).
    fn read_comment(&self, path: &[Segment], trailing: bool) -> Result<Option<String>, Error> {
        let p = to_core_path(path);
        let comment = if trailing {
            self.inner.get_trailing_comment(&p)
        } else {
            self.inner.get_leading_comment(&p)
        };
        comment_text(comment)
    }

    // ── the dangling anchor ─────────────────────────────────────────────────

    /// Add own-line comment line(s) at the END of the container at `path`'s
    /// body (an empty `path` = the document root), at the body's child depth —
    /// the third comment anchor, beside leading and trailing. `text` may be
    /// multi-line (one comment line per row), and lands below any run already
    /// there.
    ///
    /// [`Error::InvalidArgument`] when `path` names a scalar, or a flow
    /// container with no line for the run to sit on (`{ "a": 1 }` — a
    /// pretty-printed one is fine); strict JSON returns
    /// [`Error::UnsupportedFormat`].
    pub fn add_dangling_comment(&mut self, path: &[Segment], text: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.add_dangling_comment(&p, text.as_bytes());
        edit_status(status, self.format, EditTarget::Comment)
    }

    /// Remove the whole dangling run at the end of the container at `path`'s
    /// body — what [`dangling_comment`](Self::dangling_comment) reads. A no-op
    /// (still `Ok`) when there is none.
    pub fn delete_dangling_comment(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.delete_dangling_comments(&p);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Read the dangling run at the end of the container at `path`'s body
    /// (lines joined by `\n`, markers and indentation stripped). `None` when
    /// there is no run; `Some("")` for a bare marker.
    pub fn dangling_comment(&self, path: &[Segment]) -> Result<Option<String>, Error> {
        let p = to_core_path(path);
        comment_text(self.inner.get_dangling_comment(&p))
    }

    // ── comment out, and back ───────────────────────────────────────────────

    /// Turn the node at `path` into a comment run: every line of its source
    /// span takes the line marker at that line's own indentation. The entry
    /// becomes the leading block of what followed it — or, when it was last,
    /// the parent's dangling run — and the tree no longer has the node. Its own
    /// leading comment block stays above it, untouched.
    ///
    /// [`Error::InvalidArgument`] for the root, for a node that does not have
    /// its lines to itself (an item of `[a, b]`, an entry of
    /// `{ "a": 1, "b": 2 }`), and for a whole `[table]`/`[section]`, whose body
    /// is lines this op cannot see.
    pub fn comment_out(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.comment_out(&p);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Bring `line_count` lines of the LEADING comment block above the node at
    /// `path`, starting at `first_line` (0-based within that block — the block
    /// [`leading_comment`](Self::leading_comment) reports), back as entries:
    /// strip the marker and one following space from each, then reparse.
    ///
    /// Which lines look like an entry is the caller's judgement; this is the
    /// byte edit and the guarantee that it landed. If the result does not parse
    /// — or parses to a document whose OTHER nodes changed — the splice is
    /// rolled back and the call fails ([`Error::Parse`] or
    /// [`Error::UnsupportedOperation`]) with the document byte-for-byte as it
    /// was. [`Error::NotFound`] when the block has fewer lines than asked for; a
    /// `line_count` of 0 is a no-op.
    pub fn uncomment_leading(
        &mut self,
        path: &[Segment],
        first_line: usize,
        line_count: usize,
    ) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.uncomment_leading(&p, first_line, line_count);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// The dangling twin of [`uncomment_leading`](Self::uncomment_leading):
    /// the run at the end of the container at `path`'s body, which is where a
    /// commented-out LAST entry lands. Same guarantee, same errors.
    pub fn uncomment_dangling(
        &mut self,
        path: &[Segment],
        first_line: usize,
        line_count: usize,
    ) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.uncomment_dangling(&p, first_line, line_count);
        edit_status(status, self.format, EditTarget::Document)
    }

    // ── structural edits (no value) ─────────────────────────────────────────

    /// Delete the mapping entry named by `path`.
    pub fn delete_key(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.delete_key(&p);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Delete the item at `index` from the sequence at `path`.
    pub fn delete_item(&mut self, path: &[Segment], index: usize) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.remove_seq_item(&p, index);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Move the mapping entry at `src_path` to immediately before the entry at
    /// `dest_path`. Both must name keys in the same mapping. The moved entry
    /// keeps its owned comments; bytes between the two entries are preserved.
    pub fn move_key(&mut self, src_path: &[Segment], dest_path: &[Segment]) -> Result<(), Error> {
        let s = to_core_path(src_path);
        let d = to_core_path(dest_path);
        let status = self.inner.move_key(&s, &d);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Reorder the entries of the mapping at `path` (empty path = root) so
    /// `keys` come first in that order; entries whose key is not listed keep
    /// their original relative order and follow. Unknown keys are ignored. Each
    /// entry's comments and interleaved trivia are preserved.
    pub fn reorder_keys<S: AsRef<str>>(
        &mut self,
        path: &[Segment],
        keys: &[S],
    ) -> Result<(), Error> {
        let p = to_core_path(path);
        let k = to_core_keys(keys);
        let status = self.inner.reorder_keys(&p, &k);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Move the sequence item at index `from` to index `to` (array-move
    /// semantics). A block item keeps its owned comments; a flow sequence keeps
    /// its separators. No-op when `from == to`.
    pub fn move_item(&mut self, path: &[Segment], from: usize, to: usize) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.move_item(&p, from, to);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Reorder the items of the sequence at `path` so the items at `indices`
    /// (positions in the current order) come first, in that order; items not
    /// listed keep their original relative order and follow. Out-of-range
    /// indices are ignored.
    pub fn reorder_items(&mut self, path: &[Segment], indices: &[usize]) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.reorder_items(&p, indices);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Reconcile the sequence at `path` so its items are exactly `items`, while
    /// preserving the comments on items that survive the change. Items are
    /// matched to the current ones by value (kind + value, honoring
    /// multiplicity), so a kept or merely reordered item keeps its leading and
    /// trailing comments; only genuinely new values are inserted and only
    /// genuinely dropped values are deleted. The result order matches `items`.
    /// This is the comment-preserving alternative to replacing the whole list.
    ///
    /// `items` is anything iterable over `impl Into<Value>` — a `&[Value]`, a
    /// `Vec<&str>`, an array of scalars.
    ///
    /// Declines with [`Error::InvalidArgument`] (the caller should fall back to
    /// replacing the whole value) when the shape can't be safely diffed: an
    /// empty `items`, an empty current list, a non-scalar item on either side, a
    /// non-sequence target, or a format whose scalars can't stand alone (TOML).
    pub fn set_sequence<I>(&mut self, path: &[Segment], items: I) -> Result<(), Error>
    where
        I: IntoIterator,
        I::Item: Into<Value>,
    {
        let texts: Vec<String> = items
            .into_iter()
            .map(|v| value_text(&v.into(), self.format))
            .collect::<Result<_, _>>()?;
        let strs = to_core_keys(&texts);
        let p = to_core_path(path);
        let status = self.inner.set_sequence(&p, &strs);
        edit_status(status, self.format, EditTarget::Value)
    }

    // ---- whole-container ops ----
    //
    // For containers that are SCATTERED through the source: a TOML `[header]`
    // table (whose body is the lines after it, extended by every `[a.b]` header
    // elsewhere in the file), an INI `[section]`, a fig block container. Such a
    // container owns no single range to splice, so the key methods above cannot
    // address one — `delete_key`, `replace_value`, `move_key` and
    // `reorder_keys` all answer `Error::InvalidArgument` at a table path rather
    // than silently rewriting the header and rehoming the body — and these are
    // where that request goes instead.
    //
    // Support varies by format: TOML has all six, INI and `fig` the
    // delete/move/reorder three, and every other format none, answering
    // `Error::UnsupportedFormat`. YAML, JSON and the rest nest their containers
    // in one contiguous region, so the key methods already handle them.
    //
    // `body` is verbatim entry lines in the document's format
    // (`ip = "10.0.0.1"\n` for TOML), spliced and reparsed like any other edit,
    // so a body that does not parse rolls the document back.

    /// Delete the whole container at `path` — every scattered region of its
    /// subtree, leaving interleaved foreign content in place.
    pub fn delete_container(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.delete_container(&p);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Create a container at `path` with `body` as its entries, spliced past
    /// the parent's whole subtree so no existing key is reparented.
    pub fn insert_container(&mut self, path: &[Segment], body: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.insert_container(&p, body.as_bytes());
        edit_status(status, self.format, EditTarget::Value)
    }

    /// Rename the container at `path` to `new_leaf`, rewriting every line that
    /// names it: renaming `a` rewrites `[a]`, `[a.b]` and `[[a.c]]` alike.
    pub fn rename_container(&mut self, path: &[Segment], new_leaf: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.rename_container(&p, new_leaf.as_bytes());
        edit_status(status, self.format, EditTarget::Key)
    }

    /// Move the container at `src_path` before the one at `dest_path`,
    /// re-emitting its scattered fragments contiguously; interleaved foreign
    /// containers stay put. `None` moves it to the end of the document —
    /// distinct from `Some(&[])`, which names the root.
    pub fn move_container(
        &mut self,
        src_path: &[Segment],
        dest_path: Option<&[Segment]>,
    ) -> Result<(), Error> {
        let s = to_core_path(src_path);
        let d = dest_path.map(to_core_path);
        // `None` is "to EOF", distinct from `Some(&[])`, the root.
        let status = self.inner.move_container(&s, d.as_deref());
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Reorder top-level containers so those named in `order` come first, in
    /// that order, each re-emitted contiguously at the position the earliest of
    /// them currently occupies. Containers not named keep their places.
    ///
    /// Unlike [`reorder_keys`](Self::reorder_keys) this takes no path: the
    /// core reorders the document's top-level containers only, and a path that
    /// could only ever be empty would be a parameter that means nothing.
    pub fn reorder_containers<S: AsRef<str>>(&mut self, order: &[S]) -> Result<(), Error> {
        let o = to_core_keys(order);
        let status = self.inner.reorder_containers(&o);
        edit_status(status, self.format, EditTarget::Document)
    }

    /// Append an element with body `body` to the container sequence at `path` —
    /// TOML's `[[header]]` array-of-tables append.
    pub fn append_container_to_seq(&mut self, path: &[Segment], body: &str) -> Result<(), Error> {
        let p = to_core_path(path);
        let status = self.inner.append_container_to_seq(&p, body.as_bytes());
        edit_status(status, self.format, EditTarget::Value)
    }

    /// The current source. Borrows editor memory; invalidated by the next edit.
    pub fn source(&self) -> Result<&str, Error> {
        std::str::from_utf8(self.inner.source()).map_err(|_| Error::Utf8)
    }
}

/// A comment read's answer: `NotFound` is `Ok(None)` — distinguishing an
/// absent comment from a present-but-empty one (`Some(String::new())`) — and
/// any other status folds as [`Error::from_status`] folds it.
fn comment_text(comment: Result<Vec<u8>, Status>) -> Result<Option<String>, Error> {
    match comment {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| Error::Utf8),
        Err(Status::NotFound) => Ok(None),
        Err(status) => Err(Error::from_status(status).err().unwrap_or(Error::Internal)),
    }
}
