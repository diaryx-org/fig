//! Port of languages/json/src/parser.zig.
//!
//! The parser turns a JSON-formatted `&[u8]` into the node table fig hands
//! it. It pulls the string's tokens from the Tokenizer one at a time and
//! writes a row per node as it goes (`RowWriter`): a container when it
//! opens, an entry when its key is met, then the key and value — so the
//! rows are written in the pre-order fig reads them in.
//!
//! A decoded string (one with escapes) is copied into the table's buffer;
//! the parser's own copy is dropped with it.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use super::tokenizer::{Kind as TokenKind, Token, Tokenizer};
use super::Type;
use fig_kit::ast::{self, Comment, Extended, ExtKind, NodeId, NodeKind, Number, NumberKind, Style, AST};
use fig_kit::document::Document;
use fig_kit::parse_diagnostic;
use fig_kit::row_writer::RowWriter;
use fig_kit::span::Span;
use fig_kit::table::{self, Table};
use fig_kit::unicode;
use fig_kit::{err, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerKind {
    Array,
    Object,
}

/// Either an array or object in the process of being parsed.
#[derive(Debug)]
struct OpenContainer<'a> {
    id: NodeId,
    kind: ContainerKind,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    pending_key: Option<NodeId>,
    /// The pending key's entry: its row is written just before the key's,
    /// so the rows come out in pre-order and `finish` moves none of them.
    pending_entry: NodeId,
    /// The pending key's text: a slice of the input, or a decoded string —
    /// not the table's buffer, which moves.
    pending_key_text: Cow<'a, [u8]>,
    /// Object only: every key text seen so far, to warn on a repeat (see
    /// `WarningCode::DuplicateKey`). Dropped when the container closes.
    seen_keys: HashSet<Cow<'a, [u8]>>,
}

impl<'a> OpenContainer<'a> {
    fn new(id: NodeId, kind: ContainerKind) -> OpenContainer<'a> {
        OpenContainer {
            id,
            kind,
            first_child: None,
            last_child: None,
            pending_key: None,
            pending_entry: 0,
            pending_key_text: Cow::Borrowed(b""),
            seen_keys: HashSet::new(),
        }
    }
}

/// The parser's own error names (Zig's `ParseError`): `UnclosedObject`,
/// `UnclosedArray`, `UnclosedString`, `InvalidBool`, `InvalidNumber`,
/// `UnexpectedToken`, `InvalidUnicodeEscape`. Its `Error` is these, the
/// tokenizer's (`tokenizer::TOKENIZE_ERRORS`) and `OutOfMemory`.
pub const PARSE_ERRORS: &[&str] =
    &["UnclosedObject", "UnclosedArray", "UnclosedString", "InvalidBool", "InvalidNumber", "UnexpectedToken", "InvalidUnicodeEscape"];

/// The teaching message for `code` — one sentence naming the fix, same
/// contract as fig's `describe` (DESIGN.md "every diagnostic names the
/// fix"). JSON's flatter, single-state-machine grammar means many distinct
/// mistakes (a missing comma, a bad key, a stray token) all surface as the
/// same `UnexpectedToken` — so that message stays intentionally general; the
/// caret still narrows it to the exact offending token.
pub fn describe(code: Error) -> &'static str {
    match code.name() {
        "UnclosedObject" => "unclosed object; add the missing `}`",
        "UnclosedArray" => "unclosed array; add the missing `]`",
        "UnclosedString" => "unclosed string; a JSON string cannot span multiple lines — add the closing quote, or escape the newline as `\\n`",
        "InvalidBool" => "not a valid boolean; JSON's only booleans are the lowercase literals `true` and `false`",
        "InvalidNumber" => "not a valid number; a JSON number allows at most one `.` and one exponent (`e`/`E`)",
        "UnexpectedToken" => "unexpected token here; check for a missing comma, colon, key, or closing bracket/brace",
        "InvalidUnicodeEscape" => "invalid `\\u` escape; it needs exactly 4 hex digits (e.g. `\\u00e9`)",
        "MissingToken" => "missing token",
        "UnexpectedSlash" => "a `/` here must start a `//` or `/* */` comment, and strict JSON has no comments at all — use a .jsonc/.json5 file, or remove it",
        "MissingCloseBrace" => "missing `}` to close this object",
        "MissingOpenQuote" => "object keys must be quoted strings in strict JSON — write `\"key\"`, or use a .json5 file for unquoted keys",
        "MissingColon" => "missing `:` between this key and its value",
        "MissingCloseBracket" => "missing `]` to close this array",
        "LeadingZero" => "a number cannot have a leading zero; write the digits without the padding, or quote it as a string to keep the padding (e.g. a zip code)",
        "UnexpectedEndOfInput" => "the document ended before this value/token was complete",
        "UnclosedComment" => "unclosed block comment; add the closing `*/`",
        _ => "out of memory",
    }
}

/// A short (few-word) noun phrase for `code` — for a caret annotation
/// (`^ unexpected token`), same purpose as fig's `shortLabel`.
pub fn short_label(code: Error) -> &'static str {
    match code.name() {
        "UnclosedObject" => "unclosed object",
        "UnclosedArray" => "unclosed array",
        "UnclosedString" => "unclosed string",
        "InvalidBool" => "invalid boolean",
        "InvalidNumber" => "invalid number",
        "UnexpectedToken" => "unexpected token",
        "InvalidUnicodeEscape" => "invalid unicode escape",
        "MissingToken" => "missing token",
        "UnexpectedSlash" => "comments not allowed here",
        "MissingCloseBrace" => "missing `}`",
        "MissingOpenQuote" => "key must be quoted",
        "MissingColon" => "missing `:`",
        "MissingCloseBracket" => "missing `]`",
        "LeadingZero" => "leading zero",
        "UnexpectedEndOfInput" => "unexpected end of input",
        "UnclosedComment" => "unclosed comment",
        _ => "out of memory",
    }
}

/// A parse failure plus the byte span where it fired. The parser doesn't
/// thread a location through every helper's return site (`get_string`,
/// `get_number`, …); instead the main token loop knows which token it was
/// dispatching when an error propagates and anchors the diagnostic on that
/// token's whole span — less surgical than fig's per-site overrides (an
/// invalid `\u` escape mid-string points at the whole string, not the 4
/// offending hex chars), but a real location beats none, and JSON's flat
/// grammar rarely needs finer resolution. A tokenizer failure anchors on the
/// tokenizer's cursor instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Error,
    pub offset: usize,
    pub end: Option<usize>,
}

impl Diagnostic {
    /// 1-based line/column of `offset`, plus the full offending line.
    pub fn locate<'s>(&self, source: &'s [u8]) -> parse_diagnostic::Location<'s> {
        parse_diagnostic::locate_offset(source, self.offset)
    }

    /// Render `file:line:col: error: <message>` + source line + caret.
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"error", describe(self.code).as_bytes())
    }
}

/// An authoring-time lint: the document is valid JSON, but the shape is a
/// likely mistake. Collected during the parse (valid alongside a failure
/// too); the caller decides how to present it — same contract as fig's
/// `Warning`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub code: WarningCode,
    pub offset: usize,
    pub end: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    /// A key appears more than once in one object. Legal per the JSON spec
    /// (which does not forbid it), but consumers disagree on which value
    /// wins (this library's own `get_val_by_path` resolves to the FIRST;
    /// most JS-based tooling resolves to the LAST) — so one value is always
    /// silently discarded, which is rarely what a hand-edited or merged file
    /// meant.
    DuplicateKey,
}

impl Warning {
    pub fn describe_warning(code: WarningCode) -> &'static str {
        match code {
            WarningCode::DuplicateKey => "this key already appears earlier in the object; the JSON spec allows it, but most consumers keep only ONE of the two values (which one is implementation-defined) and silently discard the other — remove one of the two, or rename a key if both were intended",
        }
    }

    /// A short (few-word) noun phrase for `code` — mirrors `short_label`.
    pub fn short_label(code: WarningCode) -> &'static str {
        match code {
            WarningCode::DuplicateKey => "duplicate key",
        }
    }

    pub fn locate<'s>(&self, source: &'s [u8]) -> parse_diagnostic::Location<'s> {
        parse_diagnostic::locate_offset(source, self.offset)
    }

    /// Render `file:line:col: warning: <message>` + source line + caret.
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"warning", Self::describe_warning(self.code).as_bytes())
    }
}

/// Everything a parse reports besides the tree — mirrors fig's `Report`
/// field-for-field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub diag: Option<Diagnostic>,
    /// Every parse error, in source order — populated ONLY by the recovering
    /// entry point (`parse_collecting`), which resyncs past each failure and
    /// keeps going. The single-shot `parse`/`parse_with_report` stop at the
    /// first error and leave this empty, setting `diag` alone. When
    /// non-empty, `diag` mirrors `errors[0]`.
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    ExpectValue,

    ExpectArrayValueOrEnd,
    ExpectArrayCommaOrEnd,

    ExpectObjectKeyOrEnd,
    ExpectObjectKey,
    ExpectObjectColon,
    ExpectObjectValue,
    ExpectObjectCommaOrEnd,

    ExpectEndOfFile,
}

/// Expects "true" or "false", translates to boolean
pub fn get_bool(slice: &[u8]) -> Result<bool> {
    match slice {
        b"true" => Ok(true),
        b"false" => Ok(false),
        _ => Err(err!(InvalidBool)),
    }
}

/// Removes the quotes. If the string contains escape codes, decodes it into
/// an owned string.
pub fn get_string(format: Type, slice: &[u8]) -> Result<Cow<'_, [u8]>> {
    let json5 = format == Type::JSON5;
    // JSON5 strings may also be single-quoted; the closing quote must match.
    let quote: u8 = if !slice.is_empty() { slice[0] } else { 0 };
    let valid_quote = quote == b'"' || (json5 && quote == b'\'');
    if slice.len() < 2 || !valid_quote || slice[slice.len() - 1] != quote {
        return Err(err!(UnclosedString));
    }
    let inner = &slice[1..slice.len() - 1];

    // Fast path: no escapes, can safely point into source.
    if !inner.contains(&b'\\') {
        return Ok(Cow::Borrowed(inner));
    }

    // String contains escapes, so we need a new decoded string.
    let mut decoded: Vec<u8> = Vec::new();

    let mut i: usize = 0;
    while i < inner.len() {
        let c = inner[i];
        if c != b'\\' {
            decoded.push(c);
            i += 1;
            continue;
        }

        i += 1;
        if i >= inner.len() {
            return Err(err!(UnclosedString));
        }

        match inner[i] {
            b'"' => decoded.push(b'"'),   // double quote
            b'\\' => decoded.push(b'\\'), // backslash
            b'/' => decoded.push(b'/'),   // slash
            b'b' => decoded.push(0x08),   // backspace
            b'f' => decoded.push(0x0c),   // formfeed
            b'n' => decoded.push(b'\n'),  // newline
            b'r' => decoded.push(b'\r'),  // return
            b't' => decoded.push(b'\t'),  // tab
            // JSON5-only escapes.
            b'\'' => {
                if json5 {
                    decoded.push(b'\'')
                } else {
                    return Err(err!(UnexpectedToken));
                }
            }
            b'v' => {
                if json5 {
                    decoded.push(0x0b)
                } else {
                    return Err(err!(UnexpectedToken));
                }
            }
            b'0' => {
                if json5 {
                    decoded.push(0x00)
                } else {
                    return Err(err!(UnexpectedToken));
                }
            }
            b'x' => {
                // \xHH hex escape (one code point U+00HH)
                if !json5 {
                    return Err(err!(UnexpectedToken));
                }
                if i + 2 >= inner.len() {
                    return Err(err!(UnclosedString));
                }
                let byte = zig_parse_int(&inner[i + 1..i + 3], 0xFF).ok_or(err!(InvalidUnicodeEscape))?;
                unicode::encode_append(&mut decoded, byte).map_err(|_| err!(InvalidUnicodeEscape))?;
                i += 2;
            }
            // Line continuations: a backslash before a line terminator emits
            // nothing (the source line wraps). CRLF counts as one terminator.
            b'\n' => {
                if !json5 {
                    return Err(err!(UnexpectedToken));
                }
            }
            b'\r' => {
                if !json5 {
                    return Err(err!(UnexpectedToken));
                }
                if i + 1 < inner.len() && inner[i + 1] == b'\n' {
                    i += 1;
                }
            }
            b'u' => {
                // unicode
                // JSON \u escapes encode one UTF-16 code unit in 4 hex chars.
                if i + 4 >= inner.len() {
                    return Err(err!(UnclosedString));
                }
                let bytes = &inner[i + 1..i + 5];
                let first_unit = zig_parse_int(bytes, 0xFFFF).ok_or(err!(InvalidUnicodeEscape))?;
                let mut codepoint: u32 = first_unit;
                i += 4;

                // If the escape contains an unpaired surrogate, preserve the
                // raw source representation rather than failing.
                // JSONTestSuite treats these as implementation-defined `i_`
                // cases, and the AST cannot losslessly normalize them into
                // UTF-8.
                if unicode::is_high_surrogate(codepoint) {
                    if i + 6 >= inner.len() {
                        return Ok(Cow::Borrowed(inner));
                    }
                    if inner[i + 1] != b'\\' || inner[i + 2] != b'u' {
                        return Ok(Cow::Borrowed(inner));
                    }
                    let next_bytes = &inner[i + 3..i + 7];
                    let low_unit = zig_parse_int(next_bytes, 0xFFFF).ok_or(err!(InvalidUnicodeEscape))?;
                    if !unicode::is_low_surrogate(low_unit) {
                        return Ok(Cow::Borrowed(inner));
                    }
                    codepoint = 0x10000 + ((first_unit - 0xD800) << 10) + (low_unit - 0xDC00);
                    i += 6;
                } else if unicode::is_low_surrogate(codepoint) {
                    return Ok(Cow::Borrowed(inner));
                }

                unicode::encode_append(&mut decoded, codepoint).map_err(|_| err!(InvalidUnicodeEscape))?;
            }
            // JSON5 NonEscapeCharacter: any other escaped char is itself
            // (`\q` -> `q`). Strict JSON rejects unknown escapes.
            other => {
                if json5 {
                    decoded.push(other)
                } else {
                    return Err(err!(UnexpectedToken));
                }
            }
        }
        i += 1;
    }
    Ok(Cow::Owned(decoded))
}

/// Zig's `std.fmt.parseInt(T, buf, 16)` for an unsigned `T` whose largest
/// value is `max`: an optional `+` or `-` sign, then hex digits with `_`
/// allowed between them (never first or last). A negative value other than
/// zero overflows. `None` for any failure.
fn zig_parse_int(buf: &[u8], max: u32) -> Option<u32> {
    let (neg, digits) = match buf.first() {
        None => return None,
        Some(b'+') => (false, &buf[1..]),
        Some(b'-') => (true, &buf[1..]),
        Some(_) => (false, buf),
    };
    if digits.is_empty() {
        return None;
    }
    if digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return None;
    }
    let mut acc: u64 = 0;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d = (c as char).to_digit(16)? as u64;
        acc = acc * 16 + d;
        if acc > max as u64 {
            return None;
        }
    }
    if neg && acc != 0 {
        return None;
    }
    Some(acc as u32)
}

/// Returns lossless struct representation of a number
pub fn get_number(slice: &[u8]) -> Result<Number<'_>> {
    // JSON5 hexadecimal integers (`0xC8`, optionally signed) carry no dot
    // and no exponent; their `e`/`E` digits are part of the radix, not a
    // float exponent, so classify them before the dot/exponent heuristic.
    let body = if !slice.is_empty() && (slice[0] == b'+' || slice[0] == b'-') { &slice[1..] } else { slice };
    if body.len() >= 2 && body[0] == b'0' && (body[1] == b'x' || body[1] == b'X') {
        return Ok(Number { raw: slice, kind: NumberKind::Integer });
    }

    let num_dots = slice.iter().filter(|&&c| c == b'.').count();
    let kind = match num_dots {
        0 => {
            if slice.iter().any(|&c| c == b'e' || c == b'E') {
                NumberKind::Float
            } else {
                NumberKind::Integer
            }
        }
        1 => NumberKind::Float,
        _ => return Err(err!(InvalidNumber)),
    };
    Ok(Number { raw: slice, kind })
}

/// `input` as an `AST`, alone.
pub fn parse_abstract(input: &[u8], format: Type) -> Result<AST> {
    Ok(parse(input, format)?.ast)
}

/// `input` as a `Document`: its table written and adopted.
pub fn parse(input: &[u8], format: Type) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_into(&mut t, &source, format)?;
    RowWriter::adopt(source, &mut t)
}

/// `parse`, reporting as `parse_with_report_into` does.
pub fn parse_with_report(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_with_report_into(&mut t, &source, format, out)?;
    RowWriter::adopt(source, &mut t)
}

/// `parse`, collecting as `parse_collecting_into` does.
pub fn parse_collecting(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_collecting_into(&mut t, &source, format, out)?;
    RowWriter::adopt(source, &mut t)
}

/// Write `input`'s rows into `t`, the table fig hands a parse. Pass the
/// table's own source (`Table::source_arc`) as `input`, so that in-source
/// texts stay there.
pub fn parse_into(t: &mut Table, input: &[u8], format: Type) -> Result<()> {
    parse_impl(t, input, format, None, false)
}

/// `parse_into`, but also fills `out`: `diag` on failure (error code + byte
/// span, for `file:line:col` teaching messages), `warnings` always
/// (authoring-time lints — currently just `DuplicateKey`). Mirrors fig's
/// `Parser.parseWithReport` — the hook the CLI renders reports from.
pub fn parse_with_report_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, Some(out), false)
}

/// `parse_with_report_into`, but recovers past each parser-level error
/// (resyncing to the next safe token — see `resync`) to collect the WHOLE
/// file's diagnostics in one pass (`out.errors`, source order) rather than
/// stopping at the first. A TOKENIZER failure (anywhere in the input) is
/// never recoverable — it still stops the parse after exactly one
/// diagnostic, its own. On any error the return value is still the first
/// error code and the table is not finished; a clean parse writes it exactly
/// as `parse_with_report_into` would. `out.diag` mirrors `errors[0]`.
pub fn parse_collecting_into(t: &mut Table, input: &[u8], format: Type, out: &mut Report) -> Result<()> {
    parse_impl(t, input, format, Some(out), true)
}

fn parse_impl(t: &mut Table, input: &[u8], format: Type, out: Option<&mut Report>, recover: bool) -> Result<()> {
    let mut parser = Parser::new(RowWriter::init(t), input, recover);
    let result = parser.parse_once(format);
    // Warnings are copied out on every exit path (they are valid alongside
    // a failure too).
    if let Some(o) = out {
        o.warnings = parser.warnings.clone();
        if result.is_err() && !parser.diagnostics.is_empty() {
            o.diag = Some(parser.diagnostics[0]);
            if recover {
                o.errors = parser.diagnostics.clone();
            }
        }
    }
    result
}

/// The parser's state over one input.
pub struct Parser<'a, 't> {
    state: State,
    /// The table being written.
    w: RowWriter<'t>,
    container_stack: Vec<OpenContainer<'a>>,

    // Comment layer (JSON5/JSONC only — strict JSON never tokenizes a
    // comment). `pending_leading` buffers own-line comments until the next
    // node claims them (in `add_node`). Comment text borrows `input`
    // (comments carry no escapes).
    pending_leading: Vec<Comment<'a>>,
    /// The most recently completed value node — the candidate a same-line
    /// trailing comment binds to. Reset to `None` by a newline, so an
    /// own-line comment becomes leading instead.
    last_value_id: Option<NodeId>,

    root: Option<NodeId>,

    /// Whether the tokenizer has yielded `EndOfFile` (`pull`).
    at_eof: bool,
    /// Whether the parse failed in the tokenizer rather than the parser.
    tokenizer_failed: bool,

    input: &'a [u8],
    /// Which JSON dialect is being parsed. Gates the JSON5-only grammar
    /// (unquoted keys, trailing commas, `Infinity`/`NaN`, single-quoted
    /// strings).
    format: Type,

    /// Error-recovery mode: when set, a per-token dispatch failure is
    /// recorded into `diagnostics` and the parser resyncs to the next safe
    /// token instead of aborting — see `resync`. `OutOfMemory` is never
    /// recovered (it is not a document defect).
    recover: bool,
    /// Every parse error hit so far, in source order. Populated on EVERY
    /// failure (not just in `recover` mode).
    diagnostics: Vec<Diagnostic>,
    /// Authoring-time lints collected during the parse (see `WarningCode`).
    /// Valid alongside a failure too, same as fig's.
    warnings: Vec<Warning>,
}

impl<'a, 't> Parser<'a, 't> {
    fn new(w: RowWriter<'t>, input: &'a [u8], recover: bool) -> Parser<'a, 't> {
        Parser {
            state: State::ExpectValue,
            w,
            container_stack: Vec::new(),
            pending_leading: Vec::new(),
            last_value_id: None,
            root: None,
            at_eof: false,
            tokenizer_failed: false,
            input,
            format: Type::JSON,
            recover,
            diagnostics: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn parse_once(&mut self, kind: Type) -> Result<()> {
        self.format = kind;
        let input = self.input;
        let mut tokenizer = Tokenizer::new(input, kind);

        // The tokens are pulled one at a time rather than gathered first, but
        // a tokenizer failure anywhere still fails the parse as if no token
        // had been dispatched: it is the one diagnostic, and no warning
        // stands. A parse that stops early first tokenizes the rest to find
        // one.
        if let Err(e) = self.dispatch_all(&mut tokenizer) {
            if !self.tokenizer_failed {
                return Err(e);
            }
            // The tokenizer's cursor sits on (or just after) the offending
            // byte — precise enough for `file:line:col`.
            self.diagnostics.clear();
            self.warnings.clear();
            self.diagnostics.push(Diagnostic { code: e, offset: tokenizer.index, end: None });
            return Err(e);
        }

        if !self.diagnostics.is_empty() {
            // Recover mode collected 1+ errors: the tree may be malformed
            // (dangling open containers, no root), so don't try to build it —
            // return the first failure's code, matching the non-recovering
            // contract exactly.
            return Err(self.diagnostics[0].code);
        }

        // The root is the first row written, and every row after it names
        // its parent (`attach_child`, and the entry for its key and value).
        if self.root.is_none() {
            return Err(err!(UnexpectedToken));
        }
        self.w.finish_in_order()
    }

    /// Dispatch every token `tokenizer` yields, through `EndOfFile`. A parse
    /// error is recorded in `diagnostics`; single-shot it ends the parse,
    /// once the rest of the input is tokenized, and in `recover` mode the
    /// parse resyncs and goes on.
    fn dispatch_all(&mut self, tokenizer: &mut Tokenizer<'a>) -> Result<()> {
        let input = self.input;
        // Each node is a row, written in pre-order: room for every one up
        // front, rather than a copy each time the table outgrows itself.
        self.w.t.rows.reserve(row_bound(input));

        // A closer `resync` stopped on, to be dispatched next.
        let mut held: Option<Token> = None;
        loop {
            let token = match held.take() {
                Some(h) => h,
                None => self.pull(tokenizer)?,
            };
            match token.kind {
                // A newline closes the previous value's trailing-comment
                // window: a comment on the next line leads the next node
                // instead.
                TokenKind::Whitespace => {
                    if token.source(input).contains(&b'\n') {
                        self.last_value_id = None;
                    }
                    continue;
                }
                TokenKind::Comment => {
                    self.handle_comment(token)?;
                    continue;
                }
                _ => {}
            }

            if let Err(e) = self.dispatch_token(token) {
                if e.is("OutOfMemory") {
                    return Err(e); // never a document defect
                }
                // Anchor on the WHOLE token being dispatched when the error
                // fired.
                self.diagnostics.push(Diagnostic { code: e, offset: token.span.start, end: Some(token.span.end) });
                if !self.recover {
                    while !self.at_eof {
                        self.pull(tokenizer)?;
                    }
                    return Err(e);
                }
                let sync = self.resync(tokenizer, token)?;
                match sync.kind {
                    TokenKind::Comma => {
                        // Consume the comma ourselves and put the parser into
                        // the state it legally produces, so the next
                        // key/value parses normally from the token right
                        // after it.
                        if let Some(top) = self.container_stack.last() {
                            self.state = match top.kind {
                                ContainerKind::Array => State::ExpectArrayValueOrEnd,
                                ContainerKind::Object => State::ExpectObjectKey,
                            };
                        }
                    }
                    TokenKind::CloseBracket | TokenKind::CloseBrace => {
                        // Leave the closer for the normal dispatch to
                        // consume, but fix the state first so it's legal to
                        // see it.
                        if let Some(top) = self.container_stack.last() {
                            self.state = match top.kind {
                                ContainerKind::Array => State::ExpectArrayCommaOrEnd,
                                ContainerKind::Object => State::ExpectObjectCommaOrEnd,
                            };
                        }
                        held = Some(sync);
                    }
                    // Nothing left to resync onto (EOF): stop recovering.
                    _ => {
                        while !self.at_eof {
                            self.pull(tokenizer)?;
                        }
                        return Ok(());
                    }
                }
                continue;
            }
            if token.kind == TokenKind::EndOfFile {
                return Ok(());
            }
        }
    }

    /// The tokenizer's next token. A failure is marked as the tokenizer's
    /// (its codes overlap the parser's), and `EndOfFile` as the input's end.
    fn pull(&mut self, tokenizer: &mut Tokenizer<'a>) -> Result<Token> {
        let token = match tokenizer.next() {
            Ok(t) => t,
            Err(e) => {
                self.tokenizer_failed = true;
                return Err(e);
            }
        };
        if token.kind == TokenKind::EndOfFile {
            self.at_eof = true;
        }
        Ok(token)
    }

    /// One token's worth of the state machine.
    fn dispatch_token(&mut self, token: Token) -> Result<()> {
        use TokenKind as K;
        match self.state {
            State::ExpectValue => match token.kind {
                K::OpenBrace => {
                    let id = self.add_node(NodeKind::Mapping(None), token.span)?;
                    self.open_container(ContainerKind::Object, id);
                    self.state = State::ExpectObjectKeyOrEnd;
                }
                K::OpenBracket => {
                    let id = self.add_node(NodeKind::Sequence(None), token.span)?;
                    self.open_container(ContainerKind::Array, id);
                    self.state = State::ExpectArrayValueOrEnd;
                }
                K::Null | K::True | K::False | K::String | K::Number | K::Identifier => {
                    let id = self.add_token_node(token)?;
                    self.finish_value(id)?;
                }
                _ => return Err(err!(UnexpectedToken)),
            },

            State::ExpectArrayValueOrEnd => match token.kind {
                K::OpenBracket => {
                    let id = self.add_node(NodeKind::Sequence(None), token.span)?;
                    self.open_container(ContainerKind::Array, id);
                    self.state = State::ExpectArrayValueOrEnd;
                }
                K::OpenBrace => {
                    let id = self.add_node(NodeKind::Mapping(None), token.span)?;
                    self.open_container(ContainerKind::Object, id);
                    self.state = State::ExpectObjectKeyOrEnd;
                }
                K::Null | K::True | K::False | K::String | K::Number | K::Identifier => {
                    let id = self.add_token_node(token)?;
                    self.finish_value(id)?;
                }
                K::CloseBracket => {
                    let id = self.close_container(token.span.end)?;
                    self.finish_value(id)?;
                }
                _ => return Err(err!(UnexpectedToken)),
            },
            State::ExpectArrayCommaOrEnd => match token.kind {
                K::CloseBracket => {
                    let id = self.close_container(token.span.end)?;
                    self.finish_value(id)?;
                }
                K::Comma => {
                    // JSON5 permits a trailing comma: route to the state that
                    // also accepts `]`. Strict JSON must then see a value, so
                    // `[1,]` stays an error.
                    self.state = if self.format == Type::JSON5 { State::ExpectArrayValueOrEnd } else { State::ExpectValue };
                }
                _ => return Err(err!(UnexpectedToken)),
            },

            State::ExpectObjectKeyOrEnd => match token.kind {
                K::String | K::Identifier | K::True | K::False | K::Null => self.begin_key(token)?,
                K::CloseBrace => {
                    let id = self.close_container(token.span.end)?;
                    self.finish_value(id)?;
                }
                _ => return Err(err!(UnexpectedToken)),
            },
            State::ExpectObjectKey => match token.kind {
                K::String | K::Identifier | K::True | K::False | K::Null => self.begin_key(token)?,
                _ => return Err(err!(UnexpectedToken)),
            },
            State::ExpectObjectColon => match token.kind {
                K::Colon => self.state = State::ExpectObjectValue,
                _ => return Err(err!(UnexpectedToken)),
            },
            State::ExpectObjectValue => match token.kind {
                K::OpenBrace => {
                    let id = self.add_node(NodeKind::Mapping(None), token.span)?;
                    self.open_container(ContainerKind::Object, id);
                    self.state = State::ExpectObjectKeyOrEnd;
                }
                K::OpenBracket => {
                    let id = self.add_node(NodeKind::Sequence(None), token.span)?;
                    self.open_container(ContainerKind::Array, id);
                    self.state = State::ExpectArrayValueOrEnd;
                }
                K::Null | K::True | K::False | K::String | K::Number | K::Identifier => {
                    let id = self.add_token_node(token)?;
                    self.finish_value(id)?;
                }
                _ => return Err(err!(UnexpectedToken)),
            },
            State::ExpectObjectCommaOrEnd => match token.kind {
                K::CloseBrace => {
                    let id = self.close_container(token.span.end)?;
                    self.finish_value(id)?;
                }
                // JSON5 permits a trailing comma before `}`.
                K::Comma => {
                    self.state = if self.format == Type::JSON5 { State::ExpectObjectKeyOrEnd } else { State::ExpectObjectKey }
                }
                _ => return Err(err!(UnexpectedToken)),
            },

            State::ExpectEndOfFile => match token.kind {
                K::EndOfFile => {}
                _ => return Err(err!(UnexpectedToken)),
            },
        }
        Ok(())
    }

    /// After a parse error at `first` (only reachable in `recover` mode),
    /// pull forward for a safe resumption point: the next `,`/`]`/`}` at the
    /// SAME nesting depth as the error, tracking any `[`/`{` seen along the
    /// way so a well-formed nested value inside the garbage doesn't confuse
    /// the scan. A depth-0 closer whose kind doesn't match the currently open
    /// container is skipped like ordinary content rather than accepted (the
    /// caller resumes dispatch AT a returned closer, so a mismatched one
    /// would re-error on the same token forever). Falls back to `EndOfFile`
    /// when no such point exists before it, or when a matching closer would
    /// close a container that isn't actually open — the caller treats
    /// landing on `EndOfFile` as "stop recovering", so this always
    /// terminates.
    fn resync(&mut self, tokenizer: &mut Tokenizer<'a>, first: Token) -> Result<Token> {
        let mut depth: isize = 0;
        let mut token = first;
        loop {
            match token.kind {
                TokenKind::OpenBrace | TokenKind::OpenBracket => depth += 1,
                TokenKind::CloseBrace | TokenKind::CloseBracket => {
                    if depth == 0 {
                        let Some(top) = self.container_stack.last() else {
                            return Ok(Token::fixed(TokenKind::EndOfFile, token.span.start));
                        };
                        let matches = (token.kind == TokenKind::CloseBrace && top.kind == ContainerKind::Object)
                            || (token.kind == TokenKind::CloseBracket && top.kind == ContainerKind::Array);
                        if matches {
                            return Ok(token);
                        }
                        // Mismatched closer: garbage, keep scanning past it.
                    } else {
                        depth -= 1;
                    }
                }
                TokenKind::Comma => {
                    if depth == 0 {
                        return Ok(token);
                    }
                }
                TokenKind::EndOfFile => return Ok(token),
                _ => {}
            }
            token = self.pull(tokenizer)?;
        }
    }

    // ===============
    // PARSING HELPERS
    // ===============

    /// Write a node's row. A container's is written as soon as `[` or `{` is
    /// found, and gains its children as they complete.
    fn add_node(&mut self, kind: NodeKind<'_>, span: Span) -> Result<NodeId> {
        let id = self.w.add(kind, span)?;
        // Buffered leading comments bind to the first node opened for the
        // next child — a key (object entry), a value (array element / root),
        // or a container.
        self.claim_leading(id)?;
        Ok(id)
    }

    fn add_token_node(&mut self, token: Token) -> Result<NodeId> {
        let raw = token.source(self.input);
        let span = token.span;
        match token.kind {
            TokenKind::Null => self.add_node(NodeKind::Null, span),
            TokenKind::True | TokenKind::False => self.add_node(NodeKind::Boolean(get_bool(raw)?), span),
            TokenKind::String => {
                let s = get_string(self.format, raw)?;
                self.add_node(NodeKind::String(&s), span)
            }
            TokenKind::Number => {
                let kind = match special_number(raw) {
                    Some(k) => k,
                    None => NodeKind::Number(get_number(raw)?),
                };
                self.add_node(kind, span)
            }
            // A bare identifier is only a value when it spells
            // `Infinity`/`NaN`.
            TokenKind::Identifier => {
                let kind = special_number(raw).ok_or(err!(UnexpectedToken))?;
                self.add_node(kind, span)
            }
            _ => Err(err!(UnexpectedToken)),
        }
    }

    /// Record the pending object key from a `String` (quoted), `Identifier`
    /// (unquoted), or bare keyword (`true`/`false`/`null`) token, then
    /// expect `:`.
    fn begin_key(&mut self, token: Token) -> Result<()> {
        // Only quoted strings are legal keys in strict JSON; the rest are
        // JSON5.
        if token.kind != TokenKind::String && self.format != Type::JSON5 {
            return Err(err!(UnexpectedToken));
        }
        // The entry's row, before its key's: a placeholder `finish_value`
        // makes the keyvalue. Written straight to the table, so a buffered
        // leading comment waits for the key.
        let entry = self.w.add(NodeKind::Null, token.span)?;
        let (key, text) = self.add_key_node(token)?;
        let parent = self.container_stack.last_mut().expect("a key outside an object");
        parent.pending_key = Some(key);
        parent.pending_entry = entry;
        parent.pending_key_text = text;
        self.state = State::ExpectObjectColon;
        Ok(())
    }

    /// Build a string-valued key node. A quoted string is decoded; an
    /// identifier or keyword key is its verbatim source text.
    fn add_key_node(&mut self, token: Token) -> Result<(NodeId, Cow<'a, [u8]>)> {
        let input = self.input;
        let text: Cow<'a, [u8]> = match token.kind {
            TokenKind::String => get_string(self.format, token.source(input))?,
            TokenKind::Identifier | TokenKind::True | TokenKind::False | TokenKind::Null => Cow::Borrowed(token.source(input)),
            _ => return Err(err!(UnexpectedToken)),
        };
        let id = self.add_node(NodeKind::String(&text), token.span)?;
        Ok((id, text))
    }

    /// Attaches a completed child to the current open container.
    fn attach_child(&mut self, child_id: NodeId) -> Result<()> {
        let w = &mut self.w;
        let parent = self.container_stack.last_mut().expect("an open container");
        w.set_parent(child_id, parent.id);
        if parent.first_child.is_some() {
            w.set_next(parent.last_child.expect("a last child"), Some(child_id));
        } else {
            parent.first_child = Some(child_id);
            match parent.kind {
                ContainerKind::Array => w.set(parent.id, NodeKind::Sequence(Some(child_id)))?,
                ContainerKind::Object => w.set(parent.id, NodeKind::Mapping(Some(child_id)))?,
            }
        }
        parent.last_child = Some(child_id);
        Ok(())
    }

    fn finish_value(&mut self, value_id: NodeId) -> Result<()> {
        // This value is now the trailing-comment candidate (the value node is
        // the trailing anchor for both array elements and object entries).
        self.last_value_id = Some(value_id);
        // If there is no parent, the parsing is complete
        let Some(parent) = self.container_stack.last_mut() else {
            self.root = Some(value_id);
            self.state = State::ExpectEndOfFile;
            return Ok(());
        };

        match parent.kind {
            ContainerKind::Array => {
                self.attach_child(value_id)?;
                self.state = State::ExpectArrayCommaOrEnd;
            }
            ContainerKind::Object => {
                let key_id = parent.pending_key.ok_or(err!(UnexpectedToken))?;
                let key_text = std::mem::replace(&mut parent.pending_key_text, Cow::Borrowed(b""));
                parent.pending_key = None;

                // Authoring-time lint: JSON permits duplicate keys within one
                // object (the spec does not forbid it), but every mainstream
                // parser keeps only ONE value, so the other is silently
                // discarded — warn rather than stay silent about it.
                let key_span = self.w.span_of(key_id);
                if parent.seen_keys.contains(&key_text) {
                    self.warnings.push(Warning {
                        code: WarningCode::DuplicateKey,
                        offset: key_span.start,
                        end: Some(key_span.end),
                    });
                } else {
                    parent.seen_keys.insert(key_text);
                }

                // Every comment before the value was claimed by it, or at its
                // close, so the entry has none to take.
                let pair_id = parent.pending_entry;
                let value_span = self.w.span_of(value_id);
                self.w.set(pair_id, NodeKind::KeyValue(ast::KeyValue { key: key_id, value: value_id }))?;
                self.w.set_span(pair_id, Span::new(key_span.start, value_span.end));

                self.attach_child(pair_id)?;
                self.state = State::ExpectObjectCommaOrEnd;
            }
        }
        Ok(())
    }

    // ── comments ──────────────────────────────────────────────────────────

    /// Classify and attach the comment `token`. It trails the most recently
    /// completed value when that value's trailing window is still open
    /// (`last_value_id` set — no newline since) AND the comment is the last
    /// thing on its line (`ends_line`). Otherwise it buffers as leading for
    /// the next node. The `ends_line` test is what disambiguates
    /// `1, // trail` (trailing) from `1, /*c*/ b` (leading of `b`).
    fn handle_comment(&mut self, token: Token) -> Result<()> {
        let input = self.input;
        let c = parse_comment(token.source(input));
        let comment_start = token.span.start;
        if self.last_value_id.is_some() && ends_line(input, token) {
            let id = self.last_value_id.take().expect("checked"); // one trailing per value
            // A comment on the CLOSING line of a multi-line container (`]` /
            // `}` then `// c`) belongs at the bottom of the body, not on the
            // value's line — so it joins the container's `dangling` run
            // rather than its `trailing` (which is reserved for the opening
            // line). An inline container, or a scalar, keeps the same-line
            // `trailing`.
            if self.multiline_container(id, comment_start) {
                self.append_dangling(id, c)?;
            } else {
                self.set_trailing(id, c)?;
            }
        } else if ends_line(input, token) && !self.container_stack.is_empty() && after_open_delimiter(input, token) {
            // `[ // note` / `{ // note` — the comment rides the line the
            // container opened on, so it trails the container value (mirrors
            // YAML's `key: #`).
            let id = self.container_stack.last().expect("checked").id;
            self.set_trailing(id, c)?;
        } else {
            self.pending_leading.push(c);
        }
        Ok(())
    }

    /// Hand the buffered leading comments to node `id`. No-op when nothing
    /// is buffered.
    fn claim_leading(&mut self, id: NodeId) -> Result<()> {
        for c in self.pending_leading.drain(..) {
            self.w.comment(id, table::COMMENT_LEADING, c)?;
        }
        Ok(())
    }

    fn set_trailing(&mut self, id: NodeId, c: Comment<'_>) -> Result<()> {
        self.w.comment(id, table::COMMENT_TRAILING, c)
    }

    /// Whether `id` is a container whose opening delimiter is on an earlier
    /// line than `comment_start` — i.e. a multi-line `[ … ]`/`{ … }` whose
    /// close is on the comment's line. (An inline, single-line container
    /// returns false.)
    fn multiline_container(&self, id: NodeId, comment_start: usize) -> bool {
        match self.w.kind_of(id) {
            NodeKind::Sequence(_) | NodeKind::Mapping(_) => {}
            _ => return false,
        }
        let open = self.w.span_of(id).start;
        if comment_start <= open {
            return false;
        }
        self.input[open..comment_start].contains(&b'\n')
    }

    /// Append one comment to `id`'s `dangling` run. Used for a closing-line
    /// comment that follows the orphans already claimed at the close.
    fn append_dangling(&mut self, id: NodeId, c: Comment<'_>) -> Result<()> {
        self.w.comment(id, table::COMMENT_DANGLING, c)
    }

    /// Hand buffered orphan comments (no node followed them, e.g. before a
    /// closing delimiter or at EOF) to container `id` as its `dangling` run.
    fn claim_dangling(&mut self, id: NodeId) -> Result<()> {
        for c in self.pending_leading.drain(..) {
            self.w.comment(id, table::COMMENT_DANGLING, c)?;
        }
        Ok(())
    }

    /// Pushes stack metadata for a container node whose row is written
    fn open_container(&mut self, kind: ContainerKind, node_id: NodeId) {
        self.container_stack.push(OpenContainer::new(node_id, kind));
    }

    /// Pops the current container, patches its span end, and returns the
    /// node ID. Orphan comments buffered before the close delimiter become
    /// the container's `dangling` run.
    fn close_container(&mut self, span_end: usize) -> Result<NodeId> {
        let container = self.container_stack.pop().ok_or(err!(UnexpectedToken))?;
        let start = self.w.span_of(container.id).start;
        self.w.set_span(container.id, Span::new(start, span_end));
        self.claim_dangling(container.id)?;
        Ok(container.id)
    }
}

/// At least as many rows as `input` can make. A row is a value, a key, or
/// the entry a colon pairs them into: every entry has a `:`, and every value
/// but the root is an entry's or an item, which a `,` or its array's `[`
/// precedes — so `1 + 3·colons + commas + brackets` bounds them, counted as
/// bytes (one inside a string or comment only raises it).
fn row_bound(input: &[u8]) -> usize {
    let mut colons = 0usize;
    let mut others = 0usize;
    for &c in input {
        match c {
            b':' => colons += 1,
            b',' | b'[' => others += 1,
            _ => {}
        }
    }
    1 + 3 * colons + others
}

/// `Infinity`/`NaN` (optionally signed) lift to an extended
/// `NumberSpecial` node — no JSON number can hold a non-finite value.
/// Returns `None` otherwise.
fn special_number(raw: &[u8]) -> Option<NodeKind<'_>> {
    let body = if !raw.is_empty() && (raw[0] == b'+' || raw[0] == b'-') { &raw[1..] } else { raw };
    if body == b"Infinity" || body == b"NaN" {
        return Some(NodeKind::Extended(Extended { kind: ExtKind::NumberSpecial, text: raw }));
    }
    None
}

/// Whether the comment `token` is the last content on its source line —
/// i.e. the next significant token is a newline, a comma, or a closing
/// delimiter / EOF. A value/key (or another comment) appearing first on the
/// same line means this comment instead leads that following content.
///
/// Read from the bytes, the tokens not yet being pulled: whitespace is the
/// only token that can follow a comment with a whitespace byte, and the
/// three delimiters are one byte each.
fn ends_line(input: &[u8], token: Token) -> bool {
    for &c in &input[token.span.end..] {
        match c {
            b'\n' => return true,
            b' ' | b'\t' | b'\r' | 0x0b | 0x0c => {}
            b',' | b'}' | b']' => return true,
            _ => return false,
        }
    }
    true
}

/// Whether the most recent significant token before the comment `token`, on
/// the same line, is a container-opening `[`/`{`. Identifies a comment that
/// rides the line a container opened on (`[ // c`), which trails the
/// container value. Read from the bytes, as `ends_line` is.
fn after_open_delimiter(input: &[u8], token: Token) -> bool {
    let mut j = token.span.start;
    while j > 0 {
        j -= 1;
        match input[j] {
            b'\n' => return false, // crossed a newline → not the open line
            b' ' | b'\t' | b'\r' | 0x0b | 0x0c => {}
            b'[' | b'{' => return true,
            _ => return false,
        }
    }
    false
}

/// Zig's `std.mem.trim`.
fn trim<'s>(s: &'s [u8], set: &[u8]) -> &'s [u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && set.contains(&s[a]) {
        a += 1;
    }
    while b > a && set.contains(&s[b - 1]) {
        b -= 1;
    }
    &s[a..b]
}

/// Strip the markers from a comment token's raw bytes (which borrow
/// `input`) and classify line vs block.
fn parse_comment(raw: &[u8]) -> Comment<'_> {
    if raw.len() >= 2 && raw[1] == b'*' {
        // `/* … */` — the tokenizer guarantees the closing `*/`.
        return Comment { text: trim(&raw[2..raw.len() - 2], b" \t\r\n"), style: Style::Block };
    }
    // `// …` to end of line.
    Comment { text: trim(&raw[2..], b" \t\r"), style: Style::Line }
}

// =======
// Testing
// =======

#[cfg(test)]
mod tests {
    use super::*;
    use fig_kit::ast::{KeyValue, Literal, Node, PathSegment};

    fn test_parser(input: &[u8], expected: Literal<'_>) {
        let ast = parse_abstract(input, Type::JSON).unwrap();
        assert!(expected.matches(&ast));
    }

    fn test_parser_error(input: &[u8], expected_error: Error) {
        assert_eq!(expected_error, parse_abstract(input, Type::JSON).unwrap_err());
    }

    fn n(id: NodeId, kind: NodeKind<'static>) -> Node<'static> {
        Node { id, kind, next_sibling: None }
    }

    #[test]
    fn simple_json_document() {
        test_parser(
            br#"[{"hello":"world"}]"#,
            Literal {
                root: 0,
                nodes: &[
                    n(0, NodeKind::Sequence(Some(1))),
                    n(1, NodeKind::Mapping(Some(4))),
                    n(2, NodeKind::String(b"hello")),
                    n(3, NodeKind::String(b"world")),
                    n(4, NodeKind::KeyValue(KeyValue { key: 2, value: 3 })),
                ],
            },
        );
    }

    #[test]
    fn decodes_json_string_escapes() {
        let ast = parse_abstract(
            b"\"quote: \\\" slash: \\\\ newline: \\n tab: \\t backspace: \\b formfeed: \\f slash: \\/\"",
            Type::JSON,
        )
        .unwrap();
        let value = ast.node(ast.root).kind.as_string().unwrap();
        assert_eq!(&b"quote: \" slash: \\ newline: \n tab: \t backspace: \x08 formfeed: \x0c slash: /"[..], value);
    }

    #[test]
    fn decodes_json_unicode_escapes() {
        let ast = parse_abstract(b"\"A: \\u0041 latin: \\u00E9 clef: \\uD834\\uDD1E\"", Type::JSON).unwrap();
        let value = ast.node(ast.root).kind.as_string().unwrap();
        assert_eq!("A: A latin: é clef: 𝄞".as_bytes(), value);
    }

    #[test]
    fn decodes_escaped_object_keys() {
        let ast = parse_abstract(b"{\"he\\u006clo\":1}", Type::JSON).unwrap();
        let value = ast.get_val_by_path(&[PathSegment::Key(b"hello")]).unwrap();
        assert_eq!(b"1", value.kind.as_number().unwrap().raw);
    }

    #[test]
    fn preserves_unpaired_unicode_surrogate_escapes_as_raw_strings() {
        for (src, raw) in [
            (&b"\"\\uD800\""[..], &b"\\uD800"[..]),
            (b"\"\\uDC00\"", b"\\uDC00"),
            (b"\"\\uD800x\"", b"\\uD800x"),
            (b"\"\\uD800\\u0041\"", b"\\uD800\\u0041"),
        ] {
            test_parser(src, Literal { root: 0, nodes: &[n(0, NodeKind::String(raw))] });
        }
    }

    #[test]
    fn utf8_bom_before_document_is_ignored() {
        test_parser(b"\xEF\xBB\xBF{}", Literal { root: 0, nodes: &[n(0, NodeKind::Mapping(None))] });
    }

    #[test]
    fn object_trailing_comma_is_rejected() {
        test_parser_error(b"{\"a\":1,}", err!(UnexpectedToken));
    }

    // ── JSON5 ──────────────────────────────────────────────────────────────

    fn parse_json5(input: &[u8]) -> AST {
        parse_abstract(input, Type::JSON5).unwrap()
    }

    fn test_json5_error(input: &[u8], expected_error: Error) {
        assert_eq!(expected_error, parse_abstract(input, Type::JSON5).unwrap_err());
    }

    fn count_items(ast: &AST, container: NodeId) -> usize {
        let mut n = 0;
        let mut cur = match ast.node(container).kind {
            NodeKind::Sequence(first) | NodeKind::Mapping(first) => first,
            _ => return 0,
        };
        while let Some(id) = cur {
            n += 1;
            cur = ast.node(id).next_sibling;
        }
        n
    }

    #[test]
    fn json5_trailing_commas_accepted_and_still_rejected_in_strict_json() {
        let arr = parse_json5(b"[1,2,]");
        assert_eq!(2, count_items(&arr, arr.root));
        let obj = parse_json5(b"{a:1,}");
        assert_eq!(1, count_items(&obj, obj.root));
        // Strict JSON keeps rejecting both.
        test_parser_error(b"[1,2,]", err!(UnexpectedToken));
        test_parser_error(b"{\"a\":1,}", err!(UnexpectedToken));
    }

    #[test]
    fn json5_leading_comma_is_still_rejected() {
        test_json5_error(b"[,1]", err!(UnexpectedToken));
        test_json5_error(b"[,]", err!(UnexpectedToken));
    }

    #[test]
    fn json5_unquoted_and_keyword_object_keys() {
        let ast = parse_json5(b"{ hello: 1, $_$9: 2, while: 3, null: 4 }");
        for k in [&b"hello"[..], b"$_$9", b"while", b"null"] {
            let v = ast.get_val_by_path(&[PathSegment::Key(k)]).unwrap();
            assert!(matches!(v.kind, NodeKind::Number(_)));
        }
    }

    #[test]
    fn json5_single_quoted_strings_escapes_and_line_continuation() {
        let a = parse_json5(b"'I can\\'t'");
        assert_eq!(b"I can't", a.node(a.root).kind.as_string().unwrap());
        let b = parse_json5(b"'line 1 \\\nline 2'");
        assert_eq!(b"line 1 line 2", b.node(b.root).kind.as_string().unwrap());
    }

    #[test]
    fn json5_infinity_and_nan_become_extended_number_special() {
        for lit in [&b"Infinity"[..], b"-Infinity", b"+Infinity", b"NaN"] {
            let ast = parse_json5(lit);
            match ast.node(ast.root).kind {
                NodeKind::Extended(e) => {
                    assert_eq!(ExtKind::NumberSpecial, e.kind);
                    assert_eq!(lit, e.text);
                }
                k => panic!("not extended: {k:?}"),
            }
        }
    }

    #[test]
    fn json5_hexadecimal_leading_trailing_point_and_signed_numbers() {
        let cases: [(&[u8], NumberKind); 5] = [
            (b"0xC8", NumberKind::Integer),
            (b"0xc8e4", NumberKind::Integer),
            (b"+15", NumberKind::Integer),
            (b".5", NumberKind::Float),
            (b"5.", NumberKind::Float),
        ];
        for (raw, kind) in cases {
            let ast = parse_json5(raw);
            let n = ast.node(ast.root).kind.as_number().unwrap();
            assert_eq!(kind, n.kind);
            assert_eq!(raw, n.raw);
        }
    }

    #[test]
    fn json5_octal_and_lone_decimal_forms_are_rejected() {
        test_json5_error(b"010", err!(LeadingZero));
        test_json5_error(b"0x", err!(UnexpectedToken));
        test_json5_error(b".", err!(UnexpectedToken));
        test_json5_error(b"+098", err!(LeadingZero));
    }

    #[test]
    fn json5_captures_leading_trailing_line_and_block_comments() {
        let ast = parse_json5(
            b"{\n  // leading on a\n  a: 1, // trailing on a\n  /* block before b */\n  b: [\n    2 /* trailing block on 2 */,\n    3 // trailing on 3\n  ]\n}",
        );
        let root = ast.node(ast.root);
        let first = root.kind.as_mapping().unwrap().unwrap();
        let kv_a = ast.node(first).kind.as_keyvalue().unwrap();
        // Leading binds to the key, trailing to the value.
        assert_eq!(b"leading on a", ast.comments(kv_a.key).leading[0].text);
        assert_eq!(b"trailing on a", ast.comments(kv_a.value).trailing.unwrap().text);

        let kv_b = ast.node(ast.node(first).next_sibling.unwrap()).kind.as_keyvalue().unwrap();
        assert_eq!(b"block before b", ast.comments(kv_b.key).leading[0].text);
        assert_eq!(Style::Block, ast.comments(kv_b.key).leading[0].style);

        let seq = ast.node(kv_b.value);
        let e2 = ast.node(seq.kind.as_sequence().unwrap().unwrap());
        let e3 = ast.node(e2.next_sibling.unwrap());
        assert_eq!(b"trailing block on 2", ast.comments(e2.id).trailing.unwrap().text);
        assert_eq!(Style::Block, ast.comments(e2.id).trailing.unwrap().style);
        assert_eq!(b"trailing on 3", ast.comments(e3.id).trailing.unwrap().text);
    }

    #[test]
    fn json5_post_comma_comment_leads_the_next_entry_not_trails_the_previous() {
        // The classic ambiguity: `1, /*c*/ b` — the comment must lead `b`.
        let ast = parse_json5(b"{ a: 1, /* c */ b: 2 }");
        let first = ast.node(ast.root).kind.as_mapping().unwrap().unwrap();
        let kv_a = ast.node(first).kind.as_keyvalue().unwrap();
        let kv_b = ast.node(ast.node(first).next_sibling.unwrap()).kind.as_keyvalue().unwrap();
        assert!(ast.comments(kv_a.value).trailing.is_none());
        assert_eq!(b"c", ast.comments(kv_b.key).leading[0].text);
    }

    #[test]
    fn json5_comment_free_document_carries_no_comment_table() {
        let ast = parse_json5(b"{ a: 1, b: [2, 3] }");
        assert!(!ast.has_comments());
    }

    // ── diagnostics ────────────────────────────────────────────────────────

    #[test]
    fn parse_with_report_anchors_the_diagnostic_on_the_offending_token() {
        let mut report = Report::default();
        // `{"a": }` — the value slot is empty; the `}` is the unexpected token.
        let src = b"{\"a\": }";
        assert_eq!(err!(UnexpectedToken), parse_with_report(src, Type::JSON, &mut report).unwrap_err());
        let d = report.diag.unwrap();
        assert_eq!(err!(UnexpectedToken), d.code);
        assert_eq!(6, d.offset); // the `}`
        assert_eq!(0, report.errors.len()); // single-shot: no recovery
    }

    #[test]
    fn parse_with_report_reports_the_tokenizers_own_failures_with_a_location() {
        let mut report = Report::default();
        let src = b"{\"a\": \"unterminated}";
        assert_eq!(err!(UnclosedString), parse_with_report(src, Type::JSON, &mut report).unwrap_err());
        assert_eq!(err!(UnclosedString), report.diag.unwrap().code);
    }

    #[test]
    fn parse_collecting_reports_every_error_in_one_pass() {
        let mut report = Report::default();
        // Three malformed entries in one object, using only lexically-valid
        // tokens: a missing value, a stray number before the next comma, and
        // another stray number before the close.
        let src = b"{\"a\": , \"b\": 1 2, \"c\": 3 4}";
        assert_eq!(err!(UnexpectedToken), parse_collecting(src, Type::JSON, &mut report).unwrap_err());
        assert_eq!(3, report.errors.len());
        // Errors are recorded in source order.
        let mut last_offset = 0;
        for d in &report.errors {
            assert_eq!(err!(UnexpectedToken), d.code);
            assert!(d.offset >= last_offset);
            last_offset = d.offset;
        }
    }

    #[test]
    fn parse_collecting_still_recovers_a_trailing_well_formed_key_value_after_a_broken_one() {
        let mut report = Report::default();
        let src = b"{\"bad\": , \"good\": 42}";
        assert_eq!(err!(UnexpectedToken), parse_collecting(src, Type::JSON, &mut report).unwrap_err());
        assert_eq!(1, report.errors.len());
    }

    #[test]
    fn parse_collecting_terminates_on_a_mismatched_closer_instead_of_looping_forever() {
        let mut report = Report::default();
        assert_eq!(err!(UnexpectedToken), parse_collecting(b"{\"a\":1]", Type::JSON, &mut report).unwrap_err());
        assert!(!report.errors.is_empty());

        // The reverse shape: `}` while an ARRAY is open.
        let mut report2 = Report::default();
        assert_eq!(err!(UnexpectedToken), parse_collecting(b"[1}", Type::JSON, &mut report2).unwrap_err());
        assert!(!report2.errors.is_empty());
    }

    #[test]
    fn duplicate_object_key_warns_without_failing_the_parse() {
        let mut report = Report::default();
        parse_with_report(b"{\"a\": 1, \"a\": 2}", Type::JSON, &mut report).unwrap();
        assert_eq!(1, report.warnings.len());
        assert_eq!(WarningCode::DuplicateKey, report.warnings[0].code);
        // The warning anchors on the SECOND (repeated) key, not the first.
        assert_eq!(9, report.warnings[0].offset);
    }

    #[test]
    fn no_duplicate_key_warning_across_sibling_objects_or_for_array_elements() {
        let mut report = Report::default();
        parse_with_report(b"[{\"a\": 1}, {\"a\": 2}]", Type::JSON, &mut report).unwrap();
        assert_eq!(0, report.warnings.len());
    }

    #[test]
    fn describe_and_short_label_cover_every_error_variant() {
        assert!(!describe(err!(UnexpectedToken)).is_empty());
        assert!(!short_label(err!(LeadingZero)).is_empty());
        for name in PARSE_ERRORS.iter().chain(super::super::tokenizer::TOKENIZE_ERRORS) {
            let e = Error(*name);
            assert!(name == &"OutOfMemory" || describe(e) != "out of memory", "{name}");
            assert!(name == &"OutOfMemory" || short_label(e) != "out of memory", "{name}");
        }
    }
}
