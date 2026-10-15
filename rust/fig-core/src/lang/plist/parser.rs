//! Port of languages/plist/src/parser.zig.
//!
//! plist reader: parses Apple's XML property list format into the shared fig
//! AST. Scope: the XML variant only (`<plist>…</plist>`) — old-style ASCII
//! (NeXTSTEP/OpenStep `{ key = value; }`) and binary plist (`bplist00…`) are
//! separate, larger efforts (a different grammar entirely, and for binary, a
//! different byte-level format with no text lexer at all) and not attempted
//! here. See `PropertyList-1.0.dtd` (Apple, vendored nowhere — it's tiny and
//! quoted in full below) for the grammar this reader implements:
//!
//! ```text
//!   plist  := (array | data | date | dict | real | integer | string | true | false)
//!   dict   := (key, plistObject)*        -- alternating key/value, in order
//!   array  := (plistObject)*
//!   key, string, data, date := #PCDATA
//!   true, false := EMPTY                  -- `<true/>` / `<false/>`
//!   integer := ("+"|"-")? digit+
//!   real    := ("+"|"-")? digit+ ("." digit*)? (("e"|"E") ("+"|"-")? digit+)?
//! ```
//!
//! Lexing is delegated entirely to the XML tokenizer (`kit::xml`) — it is
//! purely lexical (tag/name/text-run tokens, no semantics), so this reader
//! shares it as-is rather than duplicating a scanner. Everything below is
//! plist-specific: the DTD-aware grammar, alternating-pair dict parsing,
//! numeric/boolean/date/data validation, and the AST shape.
//!
//! Data model (config-oriented, reader-only, much more direct than the
//! generic XML reader's since plist's DTD gives every element a fixed,
//! typed meaning — no attribute-folding, no `#text`, no repeated-child
//! collapsing):
//!   * `dict` → `mapping` (a real key/value mapping, not folded attributes).
//!     A duplicate `<key>` follows plist's/`plutil`'s own last-wins
//!     convention: the later value replaces the earlier one, kept at the
//!     earlier key's position.
//!   * `array` → `sequence`.
//!   * `string`/`integer`/`real`/`true`/`false` → the matching AST scalar.
//!     `integer`/`real` text is trimmed of surrounding whitespace but
//!     otherwise kept verbatim (no reparse-and-reformat), like TOML's numbers.
//!   * `date`/`data` → an `extended` scalar (`ExtKind::PlistDate`/`Data`,
//!     the byte string every format shares) — the same mechanism TOML
//!     datetimes and ZON enum/char literals use for a format-specific lexical
//!     type the abstract model has no scalar of its own for. `data`'s payload
//!     is stored with ALL whitespace stripped (real plist files commonly wrap
//!     base64 across many lines) so the intrinsic bytes are exactly the
//!     decoded value, per the `Extended.text` convention.
//!   * The document root is either `<plist [version="1.0"]>OBJECT</plist>`
//!     (the normal, Xcode/`plutil`-authored shape) or a bare OBJECT with no
//!     `<plist>` wrapper at all — `plutil -lint` accepts both, so this reader
//!     does too. Either way, the resulting AST root is just that object's own
//!     node (a mapping/sequence/scalar) — unlike the generic XML reader,
//!     there is no synthetic one-entry wrapper mapping, since plist objects
//!     are already typed values, not anonymous elements needing a name.
//!
//! No attributes are recognized anywhere except an optional `version` on
//! `<plist>` (ignored); any other attribute, anywhere, is `UnexpectedAttribute`.
//!
//! Errors (`ParseError`): `MissingRoot`, `MultipleRoots`, `UnexpectedText`,
//! `MismatchedTag`, `UnexpectedAttribute`, `UnknownElement`,
//! `UnexpectedToken`, `DictKeyExpected`, `DictValueMissing`,
//! `InvalidInteger`, `InvalidReal`, `InvalidDate`, `InvalidBoolean`,
//! `InvalidBase64`, `NestedElementInScalar`, `UnsupportedEntity`, and the XML
//! tokenizer's (`kit::xml`).

use std::sync::Arc;

use super::Type;
use crate::err;
use crate::kit::ast::{ExtKind, Extended, KeyValue, NodeId, NodeKind, Number, NumberKind, AST};
use crate::kit::document::Document;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::Table;
use crate::kit::xml::{self as tokenizer, Kind, Token};
use crate::Result;

type Id = NodeId;

pub struct Parser<'t> {
    pub version: Type,
    /// The table's own source, so that in-source texts stay there.
    source: Arc<[u8]>,
    tokens: Vec<Token>,
    pos: usize,
    /// The table being written.
    w: RowWriter<'t>,
}

/// `input` as an `AST`, alone.
pub fn parse_abstract(input: &[u8], format: Type) -> Result<AST> {
    Ok(parse(input, format)?.ast)
}

/// `input` as a `Document`: its table written and adopted.
pub fn parse(input: &[u8], format: Type) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_into(&mut t, format)?;
    RowWriter::adopt(source, &mut t)
}

/// Write the rows of `t`'s source into `t`, the table fig hands a parse.
pub fn parse_into(t: &mut Table, format: Type) -> Result<()> {
    let source = t.source_arc();
    let tokens = tokenizer::Tokenizer::new(&source).tokenize()?;
    let mut p = Parser { version: format, source: source.clone(), tokens, pos: 0, w: RowWriter::init(t) };

    p.skip_insignificant_whitespace()?;
    if p.cur_kind() != Kind::Lt {
        return Err(err!(MissingRoot));
    }
    let root_id = p.parse_document_root()?;

    p.skip_insignificant_whitespace()?;
    match p.cur_kind() {
        Kind::Eof => {}
        Kind::Lt | Kind::LtSlash => return Err(err!(MultipleRoots)),
        _ => return Err(err!(UnexpectedToken)),
    }

    p.w.finish(root_id)
}

impl<'t> Parser<'t> {
    /// Positioned at the root `lt`. Either a `<plist>` wrapper around exactly
    /// one object, or a bare object with no wrapper at all (both
    /// `plutil`-valid).
    fn parse_document_root(&mut self) -> Result<Id> {
        let sp = self.cur().span;
        self.advance(); // lt
        if self.cur_kind() != Kind::Name {
            return Err(err!(UnexpectedToken));
        }
        let name = self.cur_text_span();
        self.advance();

        if &self.source[name.start..name.end] != b"plist" {
            return self.parse_value_from_name(name, sp);
        }

        self.skip_plist_attributes()?;
        if self.cur_kind() == Kind::SlashGt {
            self.advance();
            return Err(err!(MissingRoot)); // `<plist/>` — no object inside
        }
        if self.cur_kind() != Kind::Gt {
            return Err(err!(UnexpectedToken));
        }
        self.advance();

        self.skip_insignificant_whitespace()?;
        if self.cur_kind() == Kind::LtSlash {
            return Err(err!(MissingRoot)); // `<plist></plist>`
        }
        if self.cur_kind() != Kind::Lt {
            return Err(err!(UnexpectedToken));
        }
        let inner = self.parse_value()?;

        self.skip_insignificant_whitespace()?;
        match self.cur_kind() {
            Kind::Lt => return Err(err!(MultipleRoots)), // a second object before `</plist>`
            Kind::LtSlash => {}
            _ => return Err(err!(UnexpectedToken)),
        }
        self.advance(); // lt_slash
        if self.cur_kind() != Kind::Name || self.cur_text() != b"plist" {
            return Err(err!(MismatchedTag));
        }
        self.advance();
        if self.cur_kind() != Kind::Gt {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        Ok(inner)
    }

    /// Skip `<plist>`'s own attributes — only `version` is recognized (and
    /// its value ignored: this reader targets the one plist format
    /// regardless).
    fn skip_plist_attributes(&mut self) -> Result<()> {
        while self.cur_kind() == Kind::Name {
            let is_version = self.cur_text() == b"version";
            self.advance();
            if !is_version {
                return Err(err!(UnexpectedAttribute));
            }
            if self.cur_kind() != Kind::Eq {
                return Err(err!(UnexpectedToken));
            }
            self.advance();
            if self.cur_kind() != Kind::AttrValue {
                return Err(err!(UnexpectedToken));
            }
            self.advance();
        }
        Ok(())
    }

    /// Parse one plistObject positioned at its opening `lt`.
    fn parse_value(&mut self) -> Result<Id> {
        let sp = self.cur().span;
        self.advance(); // lt
        if self.cur_kind() != Kind::Name {
            return Err(err!(UnexpectedToken));
        }
        let name = self.cur_text_span();
        self.advance();
        self.parse_value_from_name(name, sp)
    }

    /// Dispatch on an already-consumed opening tag name (shared by the
    /// wrapped- and bare-root cases and every recursive `parse_value` call).
    fn parse_value_from_name(&mut self, name: Span, sp: Span) -> Result<Id> {
        let source = self.source.clone();
        match &source[name.start..name.end] {
            b"dict" => self.parse_dict(sp),
            b"array" => self.parse_array(sp),
            b"string" => self.parse_string_scalar(sp),
            b"integer" => self.parse_number_scalar(sp, NumberKind::Integer),
            b"real" => self.parse_number_scalar(sp, NumberKind::Float),
            b"true" => self.parse_bool_scalar(sp, true),
            b"false" => self.parse_bool_scalar(sp, false),
            b"date" => self.parse_extended_scalar(sp, ExtKind::PlistDate),
            b"data" => self.parse_extended_scalar(sp, ExtKind::Data),
            // `key` only ever appears where `parse_dict` explicitly looks for it.
            _ => Err(err!(UnknownElement)),
        }
    }

    /// After an element's name, before its body: no attributes are ever valid
    /// on a plistObject (only `<plist>` — handled separately — carries one).
    /// Returns whether the tag self-closed (`<.../>`, i.e. empty content).
    fn expect_no_attrs_then_body(&mut self) -> Result<bool> {
        if self.cur_kind() == Kind::Name {
            return Err(err!(UnexpectedAttribute));
        }
        if self.cur_kind() == Kind::SlashGt {
            self.advance();
            return Ok(true);
        }
        if self.cur_kind() != Kind::Gt {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        Ok(false)
    }

    fn parse_dict(&mut self, sp: Span) -> Result<Id> {
        if self.expect_no_attrs_then_body()? {
            let span = self.extent(sp);
            return self.build_mapping(&[], span);
        }

        let mut entries: Vec<Id> = Vec::new();
        let mut keys: Vec<Vec<u8>> = Vec::new();

        loop {
            self.skip_insignificant_whitespace()?;
            if self.cur_kind() == Kind::LtSlash {
                break;
            }
            if self.cur_kind() != Kind::Lt {
                return Err(err!(UnexpectedToken));
            }

            let key_sp = self.cur().span;
            self.advance(); // lt
            if self.cur_kind() != Kind::Name || self.cur_text() != b"key" {
                return Err(err!(DictKeyExpected));
            }
            self.advance();
            let key_text = self.parse_element_text_content(b"key")?;
            let key_extent = self.extent(key_sp); // full `<key>…</key>`

            self.skip_insignificant_whitespace()?;
            if self.cur_kind() != Kind::Lt {
                return Err(err!(DictValueMissing));
            }
            let value_id = self.parse_value()?;

            // Last-wins duplicate handling (matches `plutil`/JSON convention):
            // overwrite the earlier entry's value in place rather than
            // appending a second entry for the same key.
            let mut replaced = false;
            for (eid, existing_key) in entries.iter().zip(keys.iter()) {
                if *existing_key == key_text {
                    // The earlier value's row is left unlinked; `finish` drops it.
                    let key_id = self.w.kind_of(*eid).as_keyvalue().expect("an entry").key;
                    self.w.set(*eid, NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }))?;
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                let key_id = self.string_node(&key_text, key_extent)?;
                // The entry span covers the whole `<key>…</key> … <value…>`
                // pair (both lines) so the editor deletes/moves an entry as a
                // unit; the value's own end was recorded full-extent by
                // `parse_value`.
                let entry_span = Span::new(key_sp.start, self.w.span_of(value_id).end);
                let kv_id = self.add_node(NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }), entry_span)?;
                entries.push(kv_id);
                keys.push(key_text);
            }
        }
        self.advance(); // lt_slash
        if self.cur_kind() != Kind::Name || self.cur_text() != b"dict" {
            return Err(err!(MismatchedTag));
        }
        self.advance();
        if self.cur_kind() != Kind::Gt {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        let span = self.extent(sp);
        self.build_mapping(&entries, span)
    }

    fn parse_array(&mut self, sp: Span) -> Result<Id> {
        if self.expect_no_attrs_then_body()? {
            let span = self.extent(sp);
            return self.build_sequence(&[], span);
        }

        let mut items: Vec<Id> = Vec::new();
        loop {
            self.skip_insignificant_whitespace()?;
            if self.cur_kind() == Kind::LtSlash {
                break;
            }
            if self.cur_kind() != Kind::Lt {
                return Err(err!(UnexpectedToken));
            }
            items.push(self.parse_value()?);
        }
        self.advance(); // lt_slash
        if self.cur_kind() != Kind::Name || self.cur_text() != b"array" {
            return Err(err!(MismatchedTag));
        }
        self.advance();
        if self.cur_kind() != Kind::Gt {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        let span = self.extent(sp);
        self.build_sequence(&items, span)
    }

    fn parse_string_scalar(&mut self, sp: Span) -> Result<Id> {
        let text = self.parse_element_text_content(b"string")?;
        let span = self.extent(sp);
        self.string_node(&text, span)
    }

    fn parse_number_scalar(&mut self, sp: Span, kind: NumberKind) -> Result<Id> {
        let tag_name: &[u8] = if kind == NumberKind::Integer { b"integer" } else { b"real" };
        let raw = self.parse_element_text_content(tag_name)?;
        let trimmed = trim(&raw, b" \t\r\n");
        match kind {
            NumberKind::Integer => {
                if !is_valid_integer_text(trimmed) {
                    return Err(err!(InvalidInteger));
                }
            }
            NumberKind::Float => {
                if !is_valid_real_text(trimmed) {
                    return Err(err!(InvalidReal));
                }
            }
        }
        let span = self.extent(sp);
        self.add_node(NodeKind::Number(Number { raw: trimmed, kind }), span)
    }

    fn parse_bool_scalar(&mut self, sp: Span, val: bool) -> Result<Id> {
        let tag_name: &[u8] = if val { b"true" } else { b"false" };
        if !self.expect_no_attrs_then_body()? {
            while self.cur_kind() != Kind::LtSlash {
                match self.cur_kind() {
                    Kind::CharData => {
                        if !is_whitespace_only(self.cur_text()) {
                            return Err(err!(InvalidBoolean));
                        }
                        self.advance();
                    }
                    Kind::Eof => return Err(err!(UnexpectedToken)),
                    _ => return Err(err!(InvalidBoolean)),
                }
            }
            self.advance(); // lt_slash
            if self.cur_kind() != Kind::Name || self.cur_text() != tag_name {
                return Err(err!(MismatchedTag));
            }
            self.advance();
            if self.cur_kind() != Kind::Gt {
                return Err(err!(UnexpectedToken));
            }
            self.advance();
        }
        let span = self.extent(sp);
        self.add_node(NodeKind::Boolean(val), span)
    }

    fn parse_extended_scalar(&mut self, sp: Span, kind: ExtKind) -> Result<Id> {
        let tag_name: &[u8] = if kind == ExtKind::PlistDate { b"date" } else { b"data" };
        let raw = self.parse_element_text_content(tag_name)?;
        let stripped;
        let text: &[u8] = if kind == ExtKind::Data {
            stripped = strip_all_whitespace(&raw);
            &stripped
        } else {
            trim(&raw, b" \t\r\n")
        };
        match kind {
            ExtKind::Data => {
                if !is_valid_base64(text) {
                    return Err(err!(InvalidBase64));
                }
            }
            ExtKind::PlistDate => {
                if !is_valid_date_text(text) {
                    return Err(err!(InvalidDate));
                }
            }
            _ => unreachable!(),
        }
        let span = self.extent(sp);
        self.add_node(NodeKind::Extended(Extended { kind, text }), span)
    }

    // ── scalar text content ──────────────────────────────────────────────

    /// Read a PCDATA-only element's decoded, owned content: entity
    /// references resolved, CDATA taken literally, no nested elements
    /// allowed. `tag_name` verifies the matching close tag.
    fn parse_element_text_content(&mut self, tag_name: &[u8]) -> Result<Vec<u8>> {
        if self.expect_no_attrs_then_body()? {
            return Ok(Vec::new());
        }

        let mut buf: Vec<u8> = Vec::new();
        while self.cur_kind() != Kind::LtSlash {
            match self.cur_kind() {
                Kind::CharData => {
                    decode_into(&mut buf, self.cur_text())?;
                    self.advance();
                }
                Kind::Cdata => {
                    buf.extend_from_slice(self.cur_text());
                    self.advance();
                }
                Kind::Lt => return Err(err!(NestedElementInScalar)),
                Kind::Eof => return Err(err!(UnexpectedToken)),
                _ => return Err(err!(UnexpectedToken)),
            }
        }
        self.advance(); // lt_slash
        if self.cur_kind() != Kind::Name || self.cur_text() != tag_name {
            return Err(err!(MismatchedTag));
        }
        self.advance();
        if self.cur_kind() != Kind::Gt {
            return Err(err!(UnexpectedToken));
        }
        self.advance();
        Ok(buf)
    }

    // ── node construction ────────────────────────────────────────────────

    /// Full-extent span for an element whose opening `<` was captured in
    /// `start_sp`, now that the whole element has been consumed:
    /// `tokens[pos-1]` is its just-consumed closing `>` / `/>`, so the span
    /// runs from the opening `<` through that close. The reader itself never
    /// needs extents (it walks the node tree, not spans); these are the
    /// edit-grade spans the editor splices against — a value node covers
    /// `<string>hi</string>`, a container `<dict>…</dict>`, so a
    /// replace/delete swaps the whole element. See `edit.rs`.
    fn extent(&self, start_sp: Span) -> Span {
        Span::new(start_sp.start, self.tokens[self.pos - 1].span.end)
    }

    fn add_node(&mut self, kind: NodeKind<'_>, span: Span) -> Result<Id> {
        self.w.add(kind, span)
    }

    fn string_node(&mut self, owned: &[u8], span: Span) -> Result<Id> {
        self.add_node(NodeKind::String(owned), span)
    }

    fn build_sequence(&mut self, items: &[Id], span: Span) -> Result<Id> {
        self.link(items);
        self.add_node(NodeKind::Sequence(items.first().copied()), span)
    }

    fn build_mapping(&mut self, entries: &[Id], span: Span) -> Result<Id> {
        self.link(entries);
        self.add_node(NodeKind::Mapping(entries.first().copied()), span)
    }

    fn link(&mut self, ids: &[Id]) {
        if ids.is_empty() {
            return;
        }
        for pair in ids.windows(2) {
            self.w.set_next(pair[0], Some(pair[1]));
        }
        self.w.set_next(ids[ids.len() - 1], None);
    }

    // ── cursor + helpers ─────────────────────────────────────────────────

    fn cur(&self) -> Token {
        self.tokens[self.pos]
    }

    fn cur_kind(&self) -> Kind {
        self.tokens[self.pos].kind
    }

    fn cur_text_span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn cur_text(&self) -> &[u8] {
        let s = self.tokens[self.pos].span;
        &self.source[s.start..s.end]
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    /// Consume whitespace-only `char_data`; a non-whitespace run, or a
    /// `cdata` section, where only markup is allowed is `UnexpectedText`.
    /// Used at the document prolog/epilog and between a `dict`/`array`'s
    /// element children — plist has no mixed content anywhere.
    fn skip_insignificant_whitespace(&mut self) -> Result<()> {
        while self.cur_kind() == Kind::CharData {
            if !is_whitespace_only(self.cur_text()) {
                return Err(err!(UnexpectedText));
            }
            self.advance();
        }
        if self.cur_kind() == Kind::Cdata {
            return Err(err!(UnexpectedText));
        }
        Ok(())
    }
}

/// Decode XML entity references (predefined `&amp; &lt; &gt; &quot; &apos;`
/// and numeric `&#dd; / &#xhh;`) into `buf`. The tokenizer hands entities
/// through undecoded, since what an entity means is the flavor's business.
fn decode_into(buf: &mut Vec<u8>, raw: &[u8]) -> Result<()> {
    let mut i = 0usize;
    while i < raw.len() {
        if raw[i] != b'&' {
            buf.push(raw[i]);
            i += 1;
            continue;
        }
        let semi = raw[i + 1..].iter().position(|&c| c == b';').map(|p| p + i + 1).ok_or(err!(UnsupportedEntity))?;
        let ent = &raw[i + 1..semi];
        if ent.is_empty() {
            return Err(err!(UnsupportedEntity));
        }
        if ent[0] == b'#' {
            let cp = parse_char_ref(ent).ok_or(err!(UnsupportedEntity))?;
            let c = char::from_u32(cp).ok_or(err!(UnsupportedEntity))?;
            let mut utf8 = [0u8; 4];
            buf.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        } else {
            let repl = match ent {
                b"amp" => b'&',
                b"lt" => b'<',
                b"gt" => b'>',
                b"quot" => b'"',
                b"apos" => b'\'',
                _ => return Err(err!(UnsupportedEntity)),
            };
            buf.push(repl);
        }
        i = semi + 1;
    }
    Ok(())
}

fn parse_char_ref(ent: &[u8]) -> Option<u32> {
    if ent.len() < 2 {
        return None;
    }
    let hex = ent[1] == b'x' || ent[1] == b'X';
    let digits = if hex { &ent[2..] } else { &ent[1..] };
    if digits.is_empty() {
        return None;
    }
    parse_int_u21(digits, if hex { 16 } else { 10 })
}

/// Zig's `std.fmt.parseInt(u21, buf, base)` for a fixed `base`: an optional
/// sign (a negative is only `-0`), `_` between digits, and no overflow past
/// 21 bits.
pub(crate) fn parse_int_u21(buf: &[u8], base: u32) -> Option<u32> {
    if buf.is_empty() {
        return None;
    }
    let (neg, digits) = match buf[0] {
        b'+' => (false, &buf[1..]),
        b'-' => (true, &buf[1..]),
        _ => (false, buf),
    };
    if digits.is_empty() || digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return None;
    }
    let mut acc: u32 = 0;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 10,
            b'A'..=b'Z' => (c - b'A') as u32 + 10,
            _ => return None,
        };
        if d >= base {
            return None;
        }
        if acc != 0 {
            acc = acc.checked_mul(base)?;
            if acc >= 1 << 21 {
                return None;
            }
        } else if neg {
            // `-d` fits an unsigned accumulator only when `d` is 0.
            if d != 0 {
                return None;
            }
            continue;
        }
        acc = if neg { acc.checked_sub(d)? } else { acc + d };
        if acc >= 1 << 21 {
            return None;
        }
    }
    Some(acc)
}

/// Every ASCII whitespace byte of `s` removed — `<data>` is commonly wrapped
/// across many indented lines, and none of that formatting is part of the
/// base64 payload.
fn strip_all_whitespace(s: &[u8]) -> Vec<u8> {
    s.iter().copied().filter(|&c| !is_ascii_whitespace(c)).collect()
}

// ── validation ──────────────────────────────────────────────────────────────

fn is_valid_integer_text(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    let i = if s[0] == b'+' || s[0] == b'-' { 1 } else { 0 };
    if i >= s.len() {
        return false;
    }
    s[i..].iter().all(|c| c.is_ascii_digit())
}

/// DTD grammar: `("+" | "-")? d+ ("."d*)? ("E" ("+" | "-") d+)?`. Lenient
/// beyond the letter of that grammar in two ways real-world writers rely on:
/// lowercase `e` is accepted, and the exponent's sign is optional — matching
/// CoreFoundation's own parser rather than the DTD comment literally.
fn is_valid_real_text(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut i = if s[0] == b'+' || s[0] == b'-' { 1 } else { 0 };
    let int_start = i;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    if i == int_start {
        return false;
    }
    if i < s.len() && s[i] == b'.' {
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        i += 1;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
    }
    i == s.len()
}

/// Standard base64 alphabet (`A-Za-z0-9+/`) with 0-2 trailing `=` padding
/// bytes; an empty string (empty `<data></data>`) is valid.
fn is_valid_base64(s: &[u8]) -> bool {
    let mut i = 0usize;
    while i < s.len() && s[i] != b'=' {
        let c = s[i];
        if !c.is_ascii_alphanumeric() && c != b'+' && c != b'/' {
            return false;
        }
        i += 1;
    }
    let mut pad = 0usize;
    while i < s.len() {
        if s[i] != b'=' {
            return false;
        }
        pad += 1;
        i += 1;
    }
    pad <= 2
}

/// Loose ISO-8601-subset check (DTD: "should conform to a subset of ISO
/// 8601... smaller units may be omitted"): every byte is a digit or one of
/// `-:.TZ+`, and the text is non-empty. Not a full calendar validation (no
/// range checks on month/day/hour, unlike TOML's stricter datetime reader) —
/// this is reader-first scope; a bogus-but-well-shaped date still round-trips
/// as the same text.
fn is_valid_date_text(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    s.iter().all(|&c| c.is_ascii_digit() || c == b'-' || c == b':' || c == b'.' || c == b'T' || c == b'Z' || c == b'+')
}

fn is_whitespace_only(s: &[u8]) -> bool {
    s.iter().all(|&c| is_ascii_whitespace(c))
}

fn is_ascii_whitespace(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\r' || c == b'\n'
}

/// `std.mem.trim(u8, s, set)`.
fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let start = s.iter().position(|c| !set.contains(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !set.contains(c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::PathSegment;

    const PLIST_OPEN: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">";

    fn doc(s: &str) -> AST {
        parse_abstract(s.as_bytes(), Type::Xml).unwrap()
    }

    fn val<'a>(ast: &'a AST, path: &[PathSegment<'_>]) -> NodeKind<'a> {
        ast.get_val_by_path(path).unwrap().kind
    }

    fn key(k: &str) -> PathSegment<'_> {
        PathSegment::Key(k.as_bytes())
    }

    // The Zig tests print through the JSON printer; here the tree is read
    // directly.

    #[test]
    fn empty_dict() {
        let ast = doc(&format!("{PLIST_OPEN}<dict/></plist>"));
        assert_eq!(NodeKind::Mapping(None), ast.node(ast.root).kind);
    }

    #[test]
    fn empty_array() {
        let ast = doc(&format!("{PLIST_OPEN}<array/></plist>"));
        assert_eq!(NodeKind::Sequence(None), ast.node(ast.root).kind);
    }

    #[test]
    fn scalars_string_integer_real_true_false() {
        let ast = doc(&format!(
            "{PLIST_OPEN}<dict>\n  <key>s</key><string>hi</string>\n  <key>i</key><integer>-42</integer>\n  <key>r</key><real>3.14</real>\n  <key>t</key><true/>\n  <key>f</key><false/>\n</dict></plist>"
        ));
        assert_eq!(b"hi", val(&ast, &[key("s")]).as_string().unwrap());
        assert_eq!(Number { raw: b"-42", kind: NumberKind::Integer }, val(&ast, &[key("i")]).as_number().unwrap());
        assert_eq!(Number { raw: b"3.14", kind: NumberKind::Float }, val(&ast, &[key("r")]).as_number().unwrap());
        assert_eq!(NodeKind::Boolean(true), val(&ast, &[key("t")]));
        assert_eq!(NodeKind::Boolean(false), val(&ast, &[key("f")]));
    }

    #[test]
    fn date_and_data_become_extended_scalars() {
        let ast = doc(&format!(
            "{PLIST_OPEN}<dict>\n  <key>d</key><date>2011-11-01T12:00:00Z</date>\n  <key>b</key><data>SGVsbG8=</data>\n</dict></plist>"
        ));
        assert_eq!(
            NodeKind::Extended(Extended { kind: ExtKind::PlistDate, text: b"2011-11-01T12:00:00Z" }),
            val(&ast, &[key("d")])
        );
        assert_eq!(NodeKind::Extended(Extended { kind: ExtKind::Data, text: b"SGVsbG8=" }), val(&ast, &[key("b")]));
    }

    #[test]
    fn data_payload_strips_embedded_whitespace() {
        let ast = doc(&format!("{PLIST_OPEN}<dict><key>b</key><data>\n  SGVs\n  bG8=\n</data></dict></plist>"));
        assert_eq!(NodeKind::Extended(Extended { kind: ExtKind::Data, text: b"SGVsbG8=" }), val(&ast, &[key("b")]));
    }

    #[test]
    fn nested_array_and_dict() {
        let ast = doc(&format!(
            "{PLIST_OPEN}<dict>\n  <key>arr</key>\n  <array>\n    <string>one</string>\n    <integer>2</integer>\n    <dict><key>k</key><string>v</string></dict>\n    <array><true/><false/></array>\n  </array>\n</dict></plist>"
        ));
        let arr = |i| PathSegment::Index(i);
        assert_eq!(b"one", val(&ast, &[key("arr"), arr(0)]).as_string().unwrap());
        assert_eq!(b"2", val(&ast, &[key("arr"), arr(1)]).as_number().unwrap().raw);
        assert_eq!(b"v", val(&ast, &[key("arr"), arr(2), key("k")]).as_string().unwrap());
        assert_eq!(NodeKind::Boolean(true), val(&ast, &[key("arr"), arr(3), arr(0)]));
        assert_eq!(NodeKind::Boolean(false), val(&ast, &[key("arr"), arr(3), arr(1)]));
    }

    #[test]
    fn duplicate_keys_last_wins_position_of_first_occurrence_kept() {
        let ast = doc(&format!("{PLIST_OPEN}<dict><key>a</key><string>1</string><key>a</key><string>2</string></dict></plist>"));
        assert_eq!(b"2", val(&ast, &[key("a")]).as_string().unwrap());
        let first = ast.node(ast.root).kind.as_mapping().unwrap().unwrap();
        assert_eq!(None, ast.node(first).next_sibling);
    }

    #[test]
    fn bare_root_object_with_no_plist_wrapper() {
        let ast = doc("<dict><key>a</key><string>b</string></dict>");
        assert_eq!(b"b", val(&ast, &[key("a")]).as_string().unwrap());
    }

    #[test]
    fn bare_root_scalar() {
        let ast = doc(&format!("{PLIST_OPEN}<string>just a string</string></plist>"));
        assert_eq!(b"just a string", ast.node(ast.root).kind.as_string().unwrap());
    }

    #[test]
    fn entities_decode_in_string_and_key_content() {
        let ast = doc(&format!("{PLIST_OPEN}<dict><key>k</key><string>x &amp; y &#65; &#x42;</string></dict></plist>"));
        assert_eq!(b"x & y A B", val(&ast, &[key("k")]).as_string().unwrap());
    }

    #[test]
    fn node_spans_cover_full_element_extents() {
        // The reader anchors nothing at a bare `<`; every node's span runs
        // from its opening `<` through its closing `>` so the editor can
        // splice whole elements. Locks the contract the editor depends on.
        let src = "<dict><key>k</key><integer>42</integer></dict>";
        let d = parse(src.as_bytes(), Type::Xml).unwrap();
        let slice = |sp: Span| &src[sp.start..sp.end];
        // root dict: whole document
        assert_eq!(src, slice(d.span_of(d.ast.root)));
        // the value node: the full `<integer>42</integer>`
        let v = d.ast.get_val_by_path(&[key("k")]).unwrap();
        assert_eq!("<integer>42</integer>", slice(d.span_of(v.id)));
        // the key node: the full `<key>k</key>`
        let k = d.ast.get_key_by_path(&[key("k")]).unwrap();
        assert_eq!("<key>k</key>", slice(d.span_of(k.id)));
        // the keyvalue entry: `<key>…</key>` through the value's close, as a unit
        let entry_id = d.ast.node(d.ast.root).kind.as_mapping().unwrap().unwrap();
        assert_eq!("<key>k</key><integer>42</integer>", slice(d.span_of(entry_id)));
    }

    #[test]
    fn errors() {
        let e = |s: &str| parse(s.as_bytes(), Type::Xml).unwrap_err();
        assert_eq!(err!(MissingRoot), e("   "));
        assert_eq!(err!(MissingRoot), e("<plist version=\"1.0\"/>"));
        assert_eq!(err!(MultipleRoots), e("<dict/><dict/>"));
        assert_eq!(err!(DictValueMissing), e("<dict><key>a</key></dict>"));
        assert_eq!(err!(DictKeyExpected), e("<dict><string>x</string><string>y</string></dict>"));
        assert_eq!(err!(MismatchedTag), e("<dict><key>a</key><string>1</dict>"));
        assert_eq!(err!(UnknownElement), e("<dict><key>a</key><foo>b</foo></dict>"));
        assert_eq!(err!(InvalidInteger), e("<integer>12x</integer>"));
        assert_eq!(err!(InvalidReal), e("<real>abc</real>"));
        assert_eq!(err!(InvalidBoolean), e("<true>x</true>"));
        assert_eq!(err!(InvalidBase64), e("<data>not base64!</data>"));
        assert_eq!(err!(NestedElementInScalar), e("<string><b/></string>"));
        assert_eq!(err!(UnsupportedEntity), e("<string>&bogus;</string>"));
        assert_eq!(err!(UnexpectedAttribute), e("<dict x=\"1\"/>"));
        assert_eq!(err!(UnexpectedText), e("<dict>junk<key>a</key><string>b</string></dict>"));
    }

    #[test]
    fn zig_parse_int_u21() {
        assert_eq!(Some(65), parse_int_u21(b"65", 10));
        assert_eq!(Some(0x42), parse_int_u21(b"42", 16));
        assert_eq!(Some(1000), parse_int_u21(b"1_000", 10));
        assert_eq!(Some(0), parse_int_u21(b"-0", 10));
        assert_eq!(None, parse_int_u21(b"-1", 10));
        assert_eq!(None, parse_int_u21(b"_1", 10));
        assert_eq!(None, parse_int_u21(b"2097152", 10));
        assert_eq!(Some(2097151), parse_int_u21(b"2097151", 10));
    }
}
