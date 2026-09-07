//! Comment-preserving editing of a config embedded in a host file.
//!
//! [`Embed`] opens the config region selected by an [`EmbedType`] — markdown
//! YAML frontmatter, JSON frontmatter, or YAML endmatter — and edits only that
//! block in its inner format. The fences and surrounding host text are left
//! byte-identical, and within the embed only the changed node's bytes move
//! (comments, key order, and formatting are preserved). This generalizes the
//! former YAML-frontmatter-only `Frontmatter`.
//!
//! Value-taking methods mirror [`crate::Editor`]: `*_value` take a [`Value`] and
//! are always available; the `serde`-gated forms accept any `Serialize`.

use std::ptr::NonNull;

use crate::editor::{Segment, borrow_str, to_ffi_keys, to_ffi_path};
use crate::error::Error;
use crate::value::{Value, value_text, value_text_with};
use crate::{Format, SerializeOptions, ffi};

/// Which embedded config to open — the flat mirror of fig's parametric
/// `Embed.Type`. The three parametric families (markdown `---<lang>`
/// frontmatter, ```` ```<lang> ```` fenced blocks, `<script type>` HTML data
/// islands) are enumerated once per format. The first four names are historical
/// (`FrontmatterJson` is the `;;;` block, `FrontmatterFig` is the ```` ```fig ````
/// fenced block); the rest are grouped by container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EmbedType {
    /// `---` … `---`/`...` bare YAML frontmatter.
    FrontmatterYaml,
    /// `;;;` … `;;;` JSON frontmatter.
    FrontmatterJson,
    /// YAML in a trailing ```` ```endmatter ```` code block.
    EndmatterYaml,
    /// ```` ```fig ```` … ```` ``` ```` fenced block (the fig authoring dialect).
    FrontmatterFig,
    /// `+++` … `+++` TOML frontmatter — the Hugo/Zola convention.
    PlusToml,
    /// ```` ```yaml ```` fenced block.
    FencedYaml,
    /// ```` ```json ```` fenced block.
    FencedJson,
    /// ```` ```toml ```` fenced block.
    FencedToml,
    /// `---json` … `---` markdown frontmatter.
    MdFrontmatterJson,
    /// `---toml` … `---` markdown frontmatter.
    MdFrontmatterToml,
    /// `---fig` … `---` markdown frontmatter.
    MdFrontmatterFig,
    /// `<script type="application/figl">` … `</script>` HTML data island.
    HtmlScriptFig,
    /// `<script type="application/yaml">` … `</script>` HTML data island.
    HtmlScriptYaml,
    /// `<script type="application/json">` … `</script>` HTML data island.
    HtmlScriptJson,
    /// `<script type="application/toml">` … `</script>` HTML data island.
    HtmlScriptToml,
    /// `<pre><code class="language-figl">` … `</code></pre>` visible code block.
    /// Content is entity-encoded; editing decodes on open and re-encodes
    /// span-aware on render, so an edit preserves every untouched byte's original
    /// encoding while canonically encoding only what changed.
    HtmlCodeFig,
    /// `<pre><code class="language-yaml">` visible code block.
    HtmlCodeYaml,
    /// `<pre><code class="language-json">` visible code block.
    HtmlCodeJson,
    /// `<pre><code class="language-toml">` visible code block.
    HtmlCodeToml,
}

impl EmbedType {
    /// The `(container, format)` pair the C ABI selects this archetype by.
    /// The Rust enum stays flat — one name per archetype is what a caller
    /// wants to write — and this is the one place the two shapes meet.
    fn parts(self) -> (ffi::FigEmbedContainer, ffi::FigFormat) {
        use ffi::FigEmbedContainer as C;
        use ffi::FigFormat as F;
        match self {
            EmbedType::FrontmatterYaml => (C::MdFrontmatter, F::Yaml),
            EmbedType::FrontmatterJson => (C::SemicolonsJson, F::Json),
            EmbedType::EndmatterYaml => (C::EndmatterYaml, F::Yaml),
            EmbedType::FrontmatterFig => (C::Fenced, F::Fig),
            EmbedType::PlusToml => (C::PlusToml, F::Toml),
            EmbedType::FencedYaml => (C::Fenced, F::Yaml),
            EmbedType::FencedJson => (C::Fenced, F::Json),
            EmbedType::FencedToml => (C::Fenced, F::Toml),
            EmbedType::MdFrontmatterJson => (C::MdFrontmatter, F::Json),
            EmbedType::MdFrontmatterToml => (C::MdFrontmatter, F::Toml),
            EmbedType::MdFrontmatterFig => (C::MdFrontmatter, F::Fig),
            EmbedType::HtmlScriptFig => (C::HtmlScript, F::Fig),
            EmbedType::HtmlScriptYaml => (C::HtmlScript, F::Yaml),
            EmbedType::HtmlScriptJson => (C::HtmlScript, F::Json),
            EmbedType::HtmlScriptToml => (C::HtmlScript, F::Toml),
            EmbedType::HtmlCodeFig => (C::HtmlCode, F::Fig),
            EmbedType::HtmlCodeYaml => (C::HtmlCode, F::Yaml),
            EmbedType::HtmlCodeJson => (C::HtmlCode, F::Json),
            EmbedType::HtmlCodeToml => (C::HtmlCode, F::Toml),
        }
    }

    /// `parts`' inverse: decode the `(container, format)` pair the library
    /// reports (from `fig_embed_detect`). `None` for a pair this binding has
    /// no name for — a newer core may know containers or embeddable formats
    /// this enum doesn't.
    fn from_parts(container: i32, format: i32) -> Option<Self> {
        use ffi::FigEmbedContainer as C;
        use ffi::FigFormat as F;
        let c = match container {
            v if v == C::MdFrontmatter as i32 => C::MdFrontmatter,
            v if v == C::Fenced as i32 => C::Fenced,
            v if v == C::HtmlScript as i32 => C::HtmlScript,
            v if v == C::HtmlCode as i32 => C::HtmlCode,
            v if v == C::SemicolonsJson as i32 => return Some(EmbedType::FrontmatterJson),
            v if v == C::PlusToml as i32 => return Some(EmbedType::PlusToml),
            v if v == C::EndmatterYaml as i32 => return Some(EmbedType::EndmatterYaml),
            _ => return None,
        };
        let f = match format {
            v if v == F::Yaml as i32 => F::Yaml,
            v if v == F::Json as i32 => F::Json,
            v if v == F::Toml as i32 => F::Toml,
            v if v == F::Fig as i32 => F::Fig,
            _ => return None,
        };
        Some(match (c, f) {
            (C::MdFrontmatter, F::Yaml) => EmbedType::FrontmatterYaml,
            (C::MdFrontmatter, F::Json) => EmbedType::MdFrontmatterJson,
            (C::MdFrontmatter, F::Toml) => EmbedType::MdFrontmatterToml,
            (C::MdFrontmatter, F::Fig) => EmbedType::MdFrontmatterFig,
            (C::Fenced, F::Yaml) => EmbedType::FencedYaml,
            (C::Fenced, F::Json) => EmbedType::FencedJson,
            (C::Fenced, F::Toml) => EmbedType::FencedToml,
            (C::Fenced, F::Fig) => EmbedType::FrontmatterFig,
            (C::HtmlScript, F::Yaml) => EmbedType::HtmlScriptYaml,
            (C::HtmlScript, F::Json) => EmbedType::HtmlScriptJson,
            (C::HtmlScript, F::Toml) => EmbedType::HtmlScriptToml,
            (C::HtmlScript, F::Fig) => EmbedType::HtmlScriptFig,
            (C::HtmlCode, F::Yaml) => EmbedType::HtmlCodeYaml,
            (C::HtmlCode, F::Json) => EmbedType::HtmlCodeJson,
            (C::HtmlCode, F::Toml) => EmbedType::HtmlCodeToml,
            (C::HtmlCode, F::Fig) => EmbedType::HtmlCodeFig,
            _ => return None,
        })
    }

    /// The inner format this archetype's content is written in (and serialized to
    /// when spliced in). Lets a caller resolve the parser to use for a detected
    /// embed's content instead of duplicating the archetype→format mapping.
    pub fn inner_format(self) -> Format {
        match self {
            EmbedType::FrontmatterYaml
            | EmbedType::EndmatterYaml
            | EmbedType::FencedYaml
            | EmbedType::HtmlScriptYaml
            | EmbedType::HtmlCodeYaml => Format::Yaml,
            EmbedType::FrontmatterJson
            | EmbedType::FencedJson
            | EmbedType::MdFrontmatterJson
            | EmbedType::HtmlScriptJson
            | EmbedType::HtmlCodeJson => Format::Json,
            EmbedType::FrontmatterFig
            | EmbedType::MdFrontmatterFig
            | EmbedType::HtmlScriptFig
            | EmbedType::HtmlCodeFig => Format::Fig,
            EmbedType::PlusToml
            | EmbedType::FencedToml
            | EmbedType::MdFrontmatterToml
            | EmbedType::HtmlScriptToml
            | EmbedType::HtmlCodeToml => Format::Toml,
        }
    }
}

/// A half-open `[start, end)` byte range within the host file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl From<ffi::FigSpan> for Span {
    fn from(s: ffi::FigSpan) -> Self {
        Span {
            start: s.start,
            end: s.end,
        }
    }
}

/// A located embed region in host-file byte coordinates (the content is not
/// parsed).
///
/// `body_before` and `body_after` are the host text on either side of the
/// block. With the three region spans they tile the source exactly:
///
/// ```text
/// body_before ++ open_fence ++ content ++ close_fence ++ body_after == source
/// ```
///
/// Every byte is in exactly one span (a leading UTF-8 BOM heads `body_before`),
/// so a caller can rebuild the host without losing one.
///
/// `body` is the historical one-sided view of the same thing: the suffix after
/// the close fence for frontmatter, the prefix before the open fence for
/// endmatter. For a mid-document block (an HTML `<script>` data island) that is
/// only ever half the host — prefer the two sides when reassembling.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Region {
    pub open_fence: Span,
    pub content: Span,
    pub close_fence: Span,
    pub body: Span,
    pub body_before: Span,
    pub body_after: Span,
}

/// The result of [`Embed::extract`]: a located [`Region`] plus the borrowed host
/// text, with helpers to slice out the embedded content and body without parsing
/// or copying.
#[derive(Clone, Copy, Debug)]
pub struct Extracted<'a> {
    source: &'a str,
    region: Region,
}

impl<'a> Extracted<'a> {
    /// The located region's byte spans.
    pub fn region(&self) -> Region {
        self.region
    }

    /// The raw config text between the fences (the embedded YAML/JSON); not parsed.
    pub fn content(&self) -> &'a str {
        &self.source[self.region.content.start..self.region.content.end]
    }

    /// The host body outside the fences (the markdown prose) — the one-sided
    /// [`Region::body`] view. See [`host_before`](Self::host_before) /
    /// [`host_after`](Self::host_after) for both sides of a mid-document block.
    pub fn body(&self) -> &'a str {
        &self.source[self.region.body.start..self.region.body.end]
    }

    /// The host text before the block — `[0, open_fence.start)`, a leading
    /// UTF-8 BOM included. Empty for frontmatter; the prose for endmatter; the
    /// `<head>` above an HTML `<script>` data island.
    pub fn host_before(&self) -> &'a str {
        &self.source[self.region.body_before.start..self.region.body_before.end]
    }

    /// The host text after the block — `[close_fence.end, source.len())`.
    /// Concatenating [`host_before`](Self::host_before), the three region
    /// slices, and this reproduces the source byte-for-byte.
    pub fn host_after(&self) -> &'a str {
        &self.source[self.region.body_after.start..self.region.body_after.end]
    }
}

/// Split an embedded region of `kind` from its host body without parsing or
/// copying — the read-only `(content, body)` twin of opening an [`Embed`].
/// `None` when `content` has no such region (or its opening fence has no close).
/// Both slices borrow `content`: the first is the text between the fences (no
/// fences), the second is the host prose outside them.
pub fn split(content: &str, kind: EmbedType) -> Option<(&str, &str)> {
    let e = Embed::extract(content, kind).ok()?;
    Some((e.content(), e.body()))
}

/// Best-effort sniff of which embed archetype `source` uses: try each known
/// archetype's OPEN delimiter and return the first that matches, or `None` when
/// `source` opens none of them. Only the open delimiter is checked — an
/// unterminated block is still *recognized* as its archetype, so a follow-up
/// [`Embed::extract`]/[`Embed::open`] surfaces the real error instead of a
/// misleading "nothing found". The counterpart to fig's `Language.detect`, for
/// embeds.
pub fn detect(source: &str) -> Option<EmbedType> {
    let mut container: std::os::raw::c_int = 0;
    let mut format: std::os::raw::c_int = 0;
    let status = unsafe {
        ffi::fig_embed_detect(source.as_ptr(), source.len(), &mut container, &mut format)
    };
    if status.0 != ffi::FigStatus::OK {
        return None;
    }
    EmbedType::from_parts(container, format)
}

/// An editor over an embedded config region of a host file.
#[derive(Debug)]
pub struct Embed {
    raw: NonNull<ffi::FigEmbed>,
    inner: Format,
}

impl Embed {
    /// Open the embed of `kind` in `host`. Returns [`Error::NotFound`] if no such
    /// region exists.
    pub fn open(host: &[u8], kind: EmbedType) -> Result<Self, Error> {
        let mut raw = std::ptr::null_mut();
        let (container, format) = kind.parts();
        let status = unsafe {
            ffi::fig_embed_open(host.as_ptr(), host.len(), container as i32, format as i32, &mut raw)
        };
        Error::from_status(status)?;
        let raw = NonNull::new(raw).ok_or(Error::Internal)?;
        Ok(Self {
            raw,
            inner: kind.inner_format(),
        })
    }

    /// Open the embed of `kind` in `host`, creating an empty region when none
    /// exists (placed per the archetype — frontmatter at the top, endmatter at
    /// the bottom) instead of failing with [`Error::NotFound`]. A subsequent
    /// [`set`](Self::set)/[`insert`](Self::insert) lands the first entry. An
    /// existing region is opened unchanged; a malformed one still errors.
    pub fn open_or_init(host: &[u8], kind: EmbedType) -> Result<Self, Error> {
        let mut raw = std::ptr::null_mut();
        let (container, format) = kind.parts();
        let status = unsafe {
            ffi::fig_embed_open_or_init(
                host.as_ptr(),
                host.len(),
                container as i32,
                format as i32,
                &mut raw,
            )
        };
        Error::from_status(status)?;
        let raw = NonNull::new(raw).ok_or(Error::Internal)?;
        Ok(Self {
            raw,
            inner: kind.inner_format(),
        })
    }

    /// Locate `kind`'s region in `content` and borrow its content/body slices
    /// without parsing or copying — the read-only counterpart to [`Embed::open`].
    /// [`Error::NotFound`] when no such region exists (or its fence is unterminated).
    /// Re-house `host`'s embedded region under a different archetype's fences:
    /// keep every host byte outside the block, and wrap `content` — the already
    /// re-serialized inner document, in `to`'s inner format — in `to`'s
    /// convention. The splice half of "convert this file's embed style"; the
    /// caller does the format conversion, fig does the fences and the placement.
    ///
    /// The block MOVES only when `to` puts it at the other end of the file
    /// (frontmatter <-> endmatter); otherwise it is re-housed exactly where it
    /// sat, so retyping to the same archetype is a byte-identical rebuild. The
    /// host text on both sides survives in file order either way, and a UTF-8
    /// BOM is re-emitted at offset 0 rather than travelling with the prose it
    /// precedes.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedOperation`] when `from` is a mid-document archetype
    /// (`HtmlScript*`, `HtmlCode*`) and `to` sits at an edge of the file:
    /// hoisting a `---` fence above `<html>` is neither valid markdown nor
    /// valid HTML, and leaving the block where it is does not make it
    /// frontmatter. Mid-document to mid-document is fine, and splices in place.
    ///
    /// [`Error::NotFound`] when `host` has no region of `from`;
    /// [`Error::Parse`] when it opens one and never closes it.
    ///
    /// ```
    /// use fig::{Embed, EmbedType};
    /// let out = Embed::retype(
    ///     "---\ntitle: hi\n---\n# body\n",
    ///     EmbedType::FrontmatterYaml,
    ///     EmbedType::PlusToml,
    ///     "title = \"hi\"\n",
    /// )?;
    /// assert_eq!(out, "+++\ntitle = \"hi\"\n+++\n# body\n");
    /// # Ok::<(), fig::Error>(())
    /// ```
    pub fn retype(
        host: &str,
        from: EmbedType,
        to: EmbedType,
        content: &str,
    ) -> Result<String, Error> {
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let (from_container, from_format) = from.parts();
        let (to_container, to_format) = to.parts();
        let status = unsafe {
            ffi::fig_embed_retype(
                host.as_ptr(),
                host.len(),
                from_container as i32,
                from_format as i32,
                to_container as i32,
                to_format as i32,
                content.as_ptr(),
                content.len(),
                &mut ptr,
                &mut len,
            )
        };
        Error::from_status(status)?;
        if ptr.is_null() {
            return Err(Error::Internal);
        }
        // fig owns the buffer until we copy it out; the sized free must run even
        // if the bytes turn out not to be UTF-8, so it is not guarded by `?`.
        let owned = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
        unsafe { ffi::fig_free(ptr, len) };
        String::from_utf8(owned).map_err(|_| Error::Utf8)
    }

    pub fn extract(content: &str, kind: EmbedType) -> Result<Extracted<'_>, Error> {
        let mut region = ffi::FigRegion {
            size: core::mem::size_of::<ffi::FigRegion>() as u32,
            ..Default::default()
        };
        let (container, format) = kind.parts();
        let status = unsafe {
            ffi::fig_embed_extract(
                content.as_ptr(),
                content.len(),
                container as i32,
                format as i32,
                &mut region,
            )
        };
        Error::from_status(status)?;
        Ok(Extracted {
            source: content,
            region: Region {
                open_fence: region.open_fence.into(),
                content: region.content.into(),
                close_fence: region.close_fence.into(),
                body: region.body.into(),
                body_before: region.body_before.into(),
                body_after: region.body_after.into(),
            },
        })
    }

    fn ptr(&self) -> *mut ffi::FigEmbed {
        self.raw.as_ptr()
    }

    // ── value edits (over `Value`) ──────────────────────────────────────────

    /// Replace the value at `path` with `value` — any `impl Into<Value>`: a
    /// scalar (`9i64`, `"x"`, `true`), a built [`Value`], or a `&Value`.
    pub fn replace_value(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
    ) -> Result<(), Error> {
        let repl = value_text(&value.into(), self.inner)?;
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_replace_val(self.ptr(), p.as_ptr(), p.len(), repl.as_ptr(), repl.len())
        };
        Error::from_status(status)
    }

    /// Replace the key at `path` with `key`.
    pub fn replace_key(&mut self, path: &[Segment], key: &str) -> Result<(), Error> {
        let repl = value_text(&Value::Str(key.to_string()), self.inner)?;
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_replace_key(self.ptr(), p.as_ptr(), p.len(), repl.as_ptr(), repl.len())
        };
        Error::from_status(status)
    }

    /// Insert `key: value` into the mapping at `path` (empty path = root).
    /// `value` is any `impl Into<Value>`.
    pub fn insert_value(
        &mut self,
        path: &[Segment],
        key: &str,
        value: impl Into<Value>,
    ) -> Result<(), Error> {
        let key_text = value_text(&Value::Str(key.to_string()), self.inner)?;
        let val = value_text(&value.into(), self.inner)?;
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_insert_key(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                key_text.as_ptr(),
                key_text.len(),
                val.as_ptr(),
                val.len(),
            )
        };
        Error::from_status(status)
    }

    /// Upsert a mapping value: replace the value at `path`, or insert it when
    /// only the trailing key is absent. Folds the common
    /// `replace_value` → (on [`Error::NotFound`]) `insert_value` two-step into a
    /// single call. `path` must end in a key (it only ever creates a mapping
    /// entry); a missing intermediate container surfaces as [`Error::NotFound`].
    pub fn set_value(&mut self, path: &[Segment], value: impl Into<Value>) -> Result<(), Error> {
        let val = value_text(&value.into(), self.inner)?;
        let p = to_ffi_path(path);
        let status =
            unsafe { ffi::fig_embed_set(self.ptr(), p.as_ptr(), p.len(), val.as_ptr(), val.len()) };
        Error::from_status(status)
    }

    // ── value edits with a layout knob (block-vs-inline containers) ─────────
    //
    // Embed twins of `Editor`'s `*_with` methods (see there): render `value`
    // with `options` so a block map/sequence lands as a nested section under
    // the target key inside the fence — the fig-frontmatter case that could
    // previously only be spliced per-key as inline flow.

    /// Replace the value at `path`, rendering `value` with `options` (a block
    /// map/sequence lands as a nested section).
    pub fn replace_value_with(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
        options: SerializeOptions,
    ) -> Result<(), Error> {
        let repl = value_text_with(&value.into(), self.inner, options)?;
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_replace_val(self.ptr(), p.as_ptr(), p.len(), repl.as_ptr(), repl.len())
        };
        Error::from_status(status)
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
        let key_text = value_text(&Value::Str(key.to_string()), self.inner)?;
        let val = value_text_with(&value.into(), self.inner, options)?;
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_insert_key(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                key_text.as_ptr(),
                key_text.len(),
                val.as_ptr(),
                val.len(),
            )
        };
        Error::from_status(status)
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
        let val = value_text_with(&value.into(), self.inner, options)?;
        let p = to_ffi_path(path);
        let status =
            unsafe { ffi::fig_embed_set(self.ptr(), p.as_ptr(), p.len(), val.as_ptr(), val.len()) };
        Error::from_status(status)
    }

    /// Append `value` (any `impl Into<Value>`) to the sequence at `path`.
    pub fn append_value(&mut self, path: &[Segment], value: impl Into<Value>) -> Result<(), Error> {
        let val = value_text(&value.into(), self.inner)?;
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_append_seq(self.ptr(), p.as_ptr(), p.len(), val.as_ptr(), val.len())
        };
        Error::from_status(status)
    }

    /// Prepend `value` (any `impl Into<Value>`) to the sequence at `path`.
    pub fn prepend_value(
        &mut self,
        path: &[Segment],
        value: impl Into<Value>,
    ) -> Result<(), Error> {
        let val = value_text(&value.into(), self.inner)?;
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_prepend_seq(self.ptr(), p.as_ptr(), p.len(), val.as_ptr(), val.len())
        };
        Error::from_status(status)
    }

    // ── value edits (serde convenience) ─────────────────────────────────────

    /// Replace the value at `path` with the serialized form of `value`.
    #[cfg(feature = "serde")]
    pub fn replace<T: serde::Serialize + ?Sized>(
        &mut self,
        path: &[Segment],
        value: &T,
    ) -> Result<(), Error> {
        self.replace_value(path, crate::ser::to_value(value)?)
    }

    /// Insert `key: value` into the mapping at `path` (empty path = root).
    #[cfg(feature = "serde")]
    pub fn insert<T: serde::Serialize + ?Sized>(
        &mut self,
        path: &[Segment],
        key: &str,
        value: &T,
    ) -> Result<(), Error> {
        self.insert_value(path, key, crate::ser::to_value(value)?)
    }

    /// Upsert: replace the value at `path`, or insert it when only the trailing
    /// key is absent (see [`set_value`](Self::set_value)).
    #[cfg(feature = "serde")]
    pub fn set<T: serde::Serialize + ?Sized>(
        &mut self,
        path: &[Segment],
        value: &T,
    ) -> Result<(), Error> {
        self.set_value(path, crate::ser::to_value(value)?)
    }

    /// Append the serialized form of `value` to the sequence at `path`.
    #[cfg(feature = "serde")]
    pub fn append<T: serde::Serialize + ?Sized>(
        &mut self,
        path: &[Segment],
        value: &T,
    ) -> Result<(), Error> {
        self.append_value(path, crate::ser::to_value(value)?)
    }

    /// Prepend the serialized form of `value` to the sequence at `path`.
    #[cfg(feature = "serde")]
    pub fn prepend<T: serde::Serialize + ?Sized>(
        &mut self,
        path: &[Segment],
        value: &T,
    ) -> Result<(), Error> {
        self.prepend_value(path, crate::ser::to_value(value)?)
    }

    // ── comment editing ─────────────────────────────────────────────────────

    /// Add an own-line comment ABOVE the node at `path`. Mirrors
    /// [`crate::Editor::add_leading_comment`] (YAML frontmatter uses `#`; JSON
    /// frontmatter is strict JSON and returns [`Error::UnsupportedFormat`]).
    pub fn add_leading_comment(&mut self, path: &[Segment], text: &str) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_add_leading_comment(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                text.as_ptr(),
                text.len(),
            )
        };
        Error::from_status(status)
    }

    /// Set the same-line trailing comment on the value at `path`. Mirrors
    /// [`crate::Editor::set_trailing_comment`].
    pub fn set_trailing_comment(&mut self, path: &[Segment], text: &str) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_set_trailing_comment(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                text.as_ptr(),
                text.len(),
            )
        };
        Error::from_status(status)
    }

    /// Remove the own-line comment block above the node at `path` (no-op if none).
    pub fn delete_leading_comments(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status =
            unsafe { ffi::fig_embed_delete_leading_comments(self.ptr(), p.as_ptr(), p.len()) };
        Error::from_status(status)
    }

    /// Remove the same-line trailing comment on the value at `path` (no-op if none).
    pub fn delete_trailing_comment(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status =
            unsafe { ffi::fig_embed_delete_trailing_comment(self.ptr(), p.as_ptr(), p.len()) };
        Error::from_status(status)
    }

    /// Read the own-line comment block above the node at `path` in the embedded
    /// config (markers stripped). `None` when absent, `Some("")` for a bare
    /// marker. Mirrors [`crate::Editor::leading_comment`].
    pub fn leading_comment(&self, path: &[Segment]) -> Result<Option<String>, Error> {
        self.read_comment(path, false)
    }

    /// Read the same-line trailing comment on the value at `path` in the embedded
    /// config (marker stripped). `None` when absent, `Some("")` for a bare marker.
    /// Mirrors [`crate::Editor::trailing_comment`].
    pub fn trailing_comment(&self, path: &[Segment]) -> Result<Option<String>, Error> {
        self.read_comment(path, true)
    }

    fn read_comment(&self, path: &[Segment], trailing: bool) -> Result<Option<String>, Error> {
        let p = to_ffi_path(path);
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        let status = unsafe {
            if trailing {
                ffi::fig_embed_get_trailing_comment(
                    self.ptr(),
                    p.as_ptr(),
                    p.len(),
                    &mut ptr,
                    &mut len,
                )
            } else {
                ffi::fig_embed_get_leading_comment(
                    self.ptr(),
                    p.as_ptr(),
                    p.len(),
                    &mut ptr,
                    &mut len,
                )
            }
        };
        if status.0 == ffi::FigStatus::NOT_FOUND {
            return Ok(None);
        }
        Error::from_status(status)?;
        Ok(Some(borrow_str(ptr, len)?.to_string()))
    }

    // ── the dangling anchor, and comment-out ────────────────────────────────

    /// Add own-line comment line(s) at the END of the container at `path`'s
    /// body in the embedded config. Mirrors
    /// [`crate::Editor::add_dangling_comment`].
    pub fn add_dangling_comment(&mut self, path: &[Segment], text: &str) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_add_dangling_comment(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                text.as_ptr(),
                text.len(),
            )
        };
        Error::from_status(status)
    }

    /// Remove the dangling run at the end of the container at `path`'s body
    /// (no-op if none). Mirrors [`crate::Editor::delete_dangling_comments`].
    pub fn delete_dangling_comments(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status =
            unsafe { ffi::fig_embed_delete_dangling_comments(self.ptr(), p.as_ptr(), p.len()) };
        Error::from_status(status)
    }

    /// Read the dangling run at the end of the container at `path`'s body.
    /// Mirrors [`crate::Editor::dangling_comment`].
    pub fn dangling_comment(&self, path: &[Segment]) -> Result<Option<String>, Error> {
        let p = to_ffi_path(path);
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        let status = unsafe {
            ffi::fig_embed_get_dangling_comment(self.ptr(), p.as_ptr(), p.len(), &mut ptr, &mut len)
        };
        if status.0 == ffi::FigStatus::NOT_FOUND {
            return Ok(None);
        }
        Error::from_status(status)?;
        Ok(Some(borrow_str(ptr, len)?.to_string()))
    }

    /// Turn the node at `path` into a comment run. Mirrors
    /// [`crate::Editor::comment_out`].
    pub fn comment_out(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe { ffi::fig_embed_comment_out(self.ptr(), p.as_ptr(), p.len()) };
        Error::from_status(status)
    }

    /// Bring `line_count` lines of the leading comment block above the node at
    /// `path` back as entries. Mirrors [`crate::Editor::uncomment_leading`],
    /// rollback guarantee included.
    pub fn uncomment_leading(
        &mut self,
        path: &[Segment],
        first_line: usize,
        line_count: usize,
    ) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_uncomment_leading(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                first_line,
                line_count,
            )
        };
        Error::from_status(status)
    }

    /// Bring `line_count` lines of the dangling run at the end of the container
    /// at `path`'s body back as entries. Mirrors
    /// [`crate::Editor::uncomment_dangling`].
    pub fn uncomment_dangling(
        &mut self,
        path: &[Segment],
        first_line: usize,
        line_count: usize,
    ) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_uncomment_dangling(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                first_line,
                line_count,
            )
        };
        Error::from_status(status)
    }

    // ── structural edits (no value) ─────────────────────────────────────────

    /// Delete the mapping entry named by `path`.
    pub fn delete(&mut self, path: &[Segment]) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe { ffi::fig_embed_delete_key(self.ptr(), p.as_ptr(), p.len()) };
        Error::from_status(status)
    }

    /// Remove the item at `index` from the sequence at `path`.
    pub fn remove_item(&mut self, path: &[Segment], index: usize) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status =
            unsafe { ffi::fig_embed_remove_seq_item(self.ptr(), p.as_ptr(), p.len(), index) };
        Error::from_status(status)
    }

    /// Move the mapping entry at `src_path` to immediately before the entry at
    /// `dest_path`. Both must name keys in the same mapping. The moved entry
    /// keeps its owned comments; bytes between the two entries are preserved.
    pub fn move_key(&mut self, src_path: &[Segment], dest_path: &[Segment]) -> Result<(), Error> {
        let s = to_ffi_path(src_path);
        let d = to_ffi_path(dest_path);
        let status = unsafe {
            ffi::fig_embed_move_key(self.ptr(), s.as_ptr(), s.len(), d.as_ptr(), d.len())
        };
        Error::from_status(status)
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
        let p = to_ffi_path(path);
        let k = to_ffi_keys(keys);
        let status = unsafe {
            ffi::fig_embed_reorder_keys(self.ptr(), p.as_ptr(), p.len(), k.as_ptr(), k.len())
        };
        Error::from_status(status)
    }

    /// Move the sequence item at index `from` to index `to` (array-move
    /// semantics). A block item keeps its owned comments; a flow sequence keeps
    /// its separators. No-op when `from == to`.
    pub fn move_item(&mut self, path: &[Segment], from: usize, to: usize) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe { ffi::fig_embed_move_item(self.ptr(), p.as_ptr(), p.len(), from, to) };
        Error::from_status(status)
    }

    /// Reorder the items of the sequence at `path` so the items at `indices`
    /// (positions in the current order) come first, in that order; items not
    /// listed keep their original relative order and follow. Out-of-range
    /// indices are ignored.
    pub fn reorder_items(&mut self, path: &[Segment], indices: &[usize]) -> Result<(), Error> {
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_reorder_items(
                self.ptr(),
                p.as_ptr(),
                p.len(),
                indices.as_ptr(),
                indices.len(),
            )
        };
        Error::from_status(status)
    }

    /// Reconcile the sequence at `path` so its items are exactly `items`, while
    /// preserving the comments on items that survive the change (see
    /// [`Editor::set_sequence`](crate::Editor::set_sequence) for the full
    /// semantics). Declines with [`Error::InvalidArgument`] when the shape can't
    /// be safely diffed.
    pub fn set_sequence(&mut self, path: &[Segment], items: &[Value]) -> Result<(), Error> {
        let texts: Vec<String> = items
            .iter()
            .map(|v| value_text(v, self.inner))
            .collect::<Result<_, _>>()?;
        let strs = to_ffi_keys(&texts);
        let p = to_ffi_path(path);
        let status = unsafe {
            ffi::fig_embed_set_sequence(self.ptr(), p.as_ptr(), p.len(), strs.as_ptr(), strs.len())
        };
        Error::from_status(status)
    }

    /// Replace the host body — the prose the config is embedded in — with `body`,
    /// keeping the fences and the current (possibly edited) content byte-identical.
    /// The body is the suffix after the close fence (frontmatter) or the prefix
    /// before the open fence (endmatter); only that side is swapped. `body` is
    /// taken verbatim (not parsed); an empty `body` clears it. Composes with the
    /// value edits — change keys, replace the body, then [`render`](Self::render)
    /// once. Takes effect at the next render.
    pub fn replace_body(&mut self, body: &str) -> Result<(), Error> {
        let status = unsafe { ffi::fig_embed_replace_body(self.ptr(), body.as_ptr(), body.len()) };
        Error::from_status(status)
    }

    /// Render the full host file with the edited embed spliced back between the
    /// (untouched) fences. Borrows handle memory; invalidated by the next call
    /// or edit. Takes `&mut self` because the render buffer is rebuilt in place.
    pub fn render(&mut self) -> Result<&str, Error> {
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        let status = unsafe { ffi::fig_embed_render(self.raw.as_ptr(), &mut ptr, &mut len) };
        Error::from_status(status)?;
        borrow_str(ptr, len)
    }
}

impl Drop for Embed {
    fn drop(&mut self) {
        unsafe { ffi::fig_embed_destroy(self.raw.as_ptr()) };
    }
}
