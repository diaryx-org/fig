//! Port of src/lsp/main.zig.
//!
//! fig-lsp — a minimal Language Server for the fig authoring dialect.
//!
//! This is a thin shell around the figl parser
//! (`languages/figl/src/parser.zig`, `fig_figl::parser`). It
//! speaks LSP JSON-RPC over stdio and, for now, does exactly one job well: on
//! every open/change it re-parses the document and publishes the parser's
//! teaching diagnostics (DESIGN.md's "every diagnostic names the fix") as
//! squiggles.
//!
//! The parser stays the single source of truth — the Tree-sitter grammar
//! under editors/ colors the text, this server is what makes the editor
//! understand it. Formatting (`fig fmt`), hover, and completion are
//! deliberately out of scope for this first cut; the didChange→parse→publish
//! loop is the spine everything else hangs off of.
//!
//! **The figl parser.** The Rust figl language is not ported yet. `diagnose`
//! is written against the API every ported language's `parser.rs` has
//! (`parse_collecting(input, format, &mut Report)`, `Report { diag, errors,
//! warnings }`, `describe(code)`, `Warning::describe_warning(code)`) and the
//! language's `DEFAULT_TYPE`, behind the `figl` feature of this crate: build
//! with `--features figl` once `lang/figl` lands, and make it a default then.
//! Without it the server runs and answers every message as the Zig one does,
//! but publishes no diagnostics.
//!
//! Input JSON is read with `fig_core::wire::json`, which reads integers as
//! `std.json` does and keeps any other number as text. Where the Zig server
//! would trip a safety check on a message of the wrong shape (a `method` that
//! is not a string, an `initialize` without an `id`, a `didOpen` whose
//! `params` is not an object), this one ignores the message.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};

use fig_core::wire::json::{self, Value};

/// Hard cap on one JSON-RPC message.
const MAX_BODY: usize = 64 * 1024 * 1024;

/// The stdin buffer: a header line longer than this is the reader's
/// `StreamTooLong`, which ends the session as any read error does.
const STDIN_BUF: usize = 64 * 1024;

fn main() {
    // Streaming on both halves of the transport, as the Zig server is
    // (`readerStreaming`/`writerStreaming`): a standard stream is read and
    // written from where it stands, never positionally.
    let stdin = io::stdin();
    let mut r = BufReader::with_capacity(STDIN_BUF, stdin.lock());
    let stdout = io::stdout();
    let mut w = io::BufWriter::with_capacity(64 * 1024, stdout.lock());

    let mut server = Server { w: &mut w, docs: HashMap::new(), diagnose };

    loop {
        // Any read error (EOF, broken pipe, …) means the client is gone — exit.
        let body = match read_message(&mut r) {
            Ok(Some(b)) => b,
            _ => break,
        };
        let Ok(parsed) = json::parse(&body) else { continue };
        match server.handle(&parsed) {
            Ok(Flow::Continue) => {}
            Ok(Flow::Exit) => break,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Read one `Content-Length`-framed JSON-RPC message. Returns the raw body
/// bytes, or `None` on a clean end-of-headers EOF.
fn read_message(r: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let eos = || io::Error::new(io::ErrorKind::UnexpectedEof, "EndOfStream");
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        r.read_until(b'\n', &mut line)?;
        if line.last() != Some(&b'\n') {
            // The stream ended before the delimiter.
            return if content_length.is_none() { Ok(None) } else { Err(eos()) };
        }
        if line.len() > STDIN_BUF {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "StreamTooLong"));
        }
        let trimmed = trim_end(&line, b"\r\n");
        if trimmed.is_empty() {
            break; // blank line ends the header block
        }
        const PREFIX: &[u8] = b"content-length:";
        if trimmed.len() >= PREFIX.len() && trimmed[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
            let num = trim(&trimmed[PREFIX.len()..], b" \t");
            content_length = std::str::from_utf8(num).ok().and_then(|s| parse_usize(s));
        }
        // Other headers (Content-Type, …) are ignored.
    }
    let Some(len) = content_length else { return Err(eos()) };
    if len > MAX_BODY {
        return Err(eos());
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

/// `std.fmt.parseInt(usize, s, 10)`: an optional `+`, then digits, which
/// may be separated by single underscores.
fn parse_usize(s: &str) -> Option<usize> {
    let digits = s.strip_prefix('+').unwrap_or(s);
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut n: usize = 0;
    for c in digits.bytes() {
        if c == b'_' {
            continue;
        }
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((c - b'0') as usize)?;
    }
    Some(n)
}

fn trim_end<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let mut end = s.len();
    while end > 0 && set.contains(&s[end - 1]) {
        end -= 1;
    }
    &s[..end]
}

fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let s = trim_end(s, set);
    let mut start = 0;
    while start < s.len() && set.contains(&s[start]) {
        start += 1;
    }
    &s[start..]
}

/// What `handle` says of the session after a message.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    /// `exit`: the client asked the server to stop.
    Exit,
}

struct Server<'w, W: Write> {
    w: &'w mut W,
    /// uri -> latest full text.
    docs: HashMap<String, String>,
    /// The parser's report over a document (`diagnose`).
    diagnose: fn(&[u8]) -> Report,
}

impl<W: Write> Server<'_, W> {
    fn handle(&mut self, msg: &Value) -> io::Result<Flow> {
        let Value::Object(obj) = msg else { return Ok(Flow::Continue) };
        let Some(Value::String(method)) = obj.get("method") else { return Ok(Flow::Continue) };
        let id = obj.get("id");
        let params = obj.get("params");

        match method.as_str() {
            "initialize" => {
                if let Some(id) = id {
                    self.respond_initialize(id)?;
                }
            }
            "shutdown" => {
                if let Some(id) = id {
                    self.respond_null(id)?;
                }
            }
            "exit" => return Ok(Flow::Exit),
            "textDocument/didOpen" => self.did_open(params)?,
            "textDocument/didChange" => self.did_change(params)?,
            "textDocument/didClose" => self.did_close(params)?,
            _ => {
                // Any other REQUEST (has an id) must get a reply so the client
                // isn't left hanging; notifications we don't know are ignored.
                if let Some(req_id) = id {
                    self.respond_null(req_id)?;
                }
            }
        }
        Ok(Flow::Continue)
    }

    // ─── lifecycle ───

    fn respond_initialize(&mut self, id: &Value) -> io::Result<()> {
        // textDocumentSync = 1 (Full): each didChange carries the whole document.
        let mut b = Vec::new();
        b.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":");
        write_id(&mut b, id);
        b.extend_from_slice(
            b",\"result\":{\"capabilities\":{\"textDocumentSync\":1},\"serverInfo\":{\"name\":\"fig-lsp\",\"version\":\"0.0.1\"}}}",
        );
        self.send(&b)
    }

    fn respond_null(&mut self, id: &Value) -> io::Result<()> {
        let mut b = Vec::new();
        b.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":");
        write_id(&mut b, id);
        b.extend_from_slice(b",\"result\":null}");
        self.send(&b)
    }

    // ─── text sync ───

    fn did_open(&mut self, params: Option<&Value>) -> io::Result<()> {
        let Some(Value::Object(p)) = params else { return Ok(()) };
        let Some(Value::Object(td)) = p.get("textDocument") else { return Ok(()) };
        let Some(Value::String(uri)) = td.get("uri") else { return Ok(()) };
        let Some(Value::String(text)) = td.get("text") else { return Ok(()) };
        self.store(uri, text);
        self.publish(uri, text)
    }

    fn did_change(&mut self, params: Option<&Value>) -> io::Result<()> {
        let Some(Value::Object(p)) = params else { return Ok(()) };
        let Some(Value::Object(td)) = p.get("textDocument") else { return Ok(()) };
        let Some(Value::String(uri)) = td.get("uri") else { return Ok(()) };
        let Some(Value::Array(changes)) = p.get("contentChanges") else { return Ok(()) };
        // Full sync: the last change holds the entire new document.
        let Some(last) = changes.last() else { return Ok(()) };
        let Some(Value::String(text)) = last.get("text") else { return Ok(()) };
        self.store(uri, text);
        self.publish(uri, text)
    }

    fn did_close(&mut self, params: Option<&Value>) -> io::Result<()> {
        let Some(Value::Object(p)) = params else { return Ok(()) };
        let Some(Value::Object(td)) = p.get("textDocument") else { return Ok(()) };
        let Some(Value::String(uri)) = td.get("uri") else { return Ok(()) };
        self.docs.remove(uri);
        // Clear any squiggles the client is still showing for this file.
        self.publish_empty(uri)
    }

    /// Replace the stored text for `uri`.
    fn store(&mut self, uri: &str, text: &str) {
        self.docs.insert(uri.to_owned(), text.to_owned());
    }

    // ─── diagnostics ───

    /// Parse `text`; publish the parser's report — the hard error (if any)
    /// as severity 1 (Error) plus the authoring warnings as severity 2
    /// (Warning), or clear everything on a clean, lint-free parse. Warnings
    /// collected before a failure still publish alongside the error.
    fn publish(&mut self, uri: &str, text: &str) -> io::Result<()> {
        let source = text.as_bytes();
        let report = (self.diagnose)(source);

        let mut b = Vec::new();
        b.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":");
        write_json_string(&mut b, uri.as_bytes());
        b.extend_from_slice(b",\"diagnostics\":[");
        let mut first = true;
        // `parse_collecting` fills `errors` with every failure (source
        // order); publish each as its own Error squiggle. `diag` is the
        // fallback for a non-recovering report (mirrors `errors[0]` when both
        // are present).
        if !report.errors.is_empty() {
            for d in &report.errors {
                if !first {
                    b.push(b',');
                }
                first = false;
                write_diagnostic(&mut b, source, d.offset, d.end, 1, &d.code, d.message);
            }
        } else if let Some(d) = &report.diag {
            write_diagnostic(&mut b, source, d.offset, d.end, 1, &d.code, d.message);
            first = false;
        }
        for wn in &report.warnings {
            if !first {
                b.push(b',');
            }
            first = false;
            write_diagnostic(&mut b, source, wn.offset, wn.end, 2, &wn.code, wn.message);
        }
        b.extend_from_slice(b"]}}");
        self.send(&b)
    }

    fn publish_empty(&mut self, uri: &str) -> io::Result<()> {
        let mut b = Vec::new();
        b.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":");
        write_json_string(&mut b, uri.as_bytes());
        b.extend_from_slice(b",\"diagnostics\":[]}}");
        self.send(&b)
    }

    // ─── framing ───

    fn send(&mut self, body: &[u8]) -> io::Result<()> {
        write!(self.w, "Content-Length: {}\r\n\r\n", body.len())?;
        self.w.write_all(body)?;
        self.w.flush()
    }
}

// ─── the parser's report ───

/// One diagnostic as the server publishes it: where, the code (the Zig
/// error's name, or the warning's tag), and the parser's teaching message.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Diag {
    offset: usize,
    end: Option<usize>,
    code: String,
    message: &'static str,
}

/// The figl parser's `Report`, as the server reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Report {
    diag: Option<Diag>,
    errors: Vec<Diag>,
    warnings: Vec<Diag>,
}

/// Run the parser purely for its report (error diagnostics + warnings). The
/// AST is thrown away (this is a validity check).
#[cfg(feature = "figl")]
fn diagnose(text: &[u8]) -> Report {
    use fig_figl::{self as figl, parser};
    let mut report = parser::Report::default();
    // Recover past each error so the server squiggles the WHOLE file at
    // once, not just the first mistake (`report.errors`, source order).
    let _ = parser::parse_collecting(text, figl::Type::Fig, &mut report);
    let error = |d: &parser::Diagnostic| Diag { offset: d.offset, end: d.end, code: d.code.name().to_owned(), message: parser::describe(d.code) };
    Report {
        diag: report.diag.as_ref().map(error),
        errors: report.errors.iter().map(error).collect(),
        warnings: report
            .warnings
            .iter()
            .map(|w| Diag {
                offset: w.offset,
                end: w.end,
                code: tag_name(&format!("{:?}", w.code)),
                message: parser::Warning::describe_warning(w.code),
            })
            .collect(),
    }
}

/// Without the figl parser (see the module doc): a report of nothing.
#[cfg(not(feature = "figl"))]
fn diagnose(_text: &[u8]) -> Report {
    Report::default()
}

/// A Rust variant's name as Zig spells the enum tag: `StringLooksLikeLiteral`
/// is `string_looks_like_literal` (`@tagName`).
#[cfg_attr(not(feature = "figl"), allow(dead_code))]
fn tag_name(variant: &str) -> String {
    let mut out = String::new();
    for (i, c) in variant.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

// ─── position math (LSP positions are 0-based, UTF-16 code units) ───

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    line: usize,
    character: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    start: Position,
    end: Position,
}

/// `std.unicode.utf8ByteSequenceLength`.
fn utf8_byte_sequence_length(b: u8) -> Option<usize> {
    match b {
        0x00..=0x7f => Some(1),
        0xc0..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf7 => Some(4),
        _ => None,
    }
}

/// `std.unicode.utf8Decode`: the codepoint of one whole sequence, or `None`
/// for a bad continuation byte, an overlong encoding, a surrogate half or a
/// codepoint past U+10FFFF.
fn utf8_decode(s: &[u8]) -> Option<u32> {
    let cont = |b: u8| if b & 0xc0 == 0x80 { Some((b & 0x3f) as u32) } else { None };
    match s.len() {
        1 => Some(s[0] as u32),
        2 => {
            let cp = ((s[0] & 0x1f) as u32) << 6 | cont(s[1])?;
            (cp >= 0x80).then_some(cp)
        }
        3 => {
            let cp = ((s[0] & 0x0f) as u32) << 12 | cont(s[1])? << 6 | cont(s[2])?;
            (cp >= 0x800 && !(0xd800..=0xdfff).contains(&cp)).then_some(cp)
        }
        4 => {
            let cp = ((s[0] & 0x07) as u32) << 18 | cont(s[1])? << 12 | cont(s[2])? << 6 | cont(s[3])?;
            (cp >= 0x10000 && cp <= 0x10ffff).then_some(cp)
        }
        _ => None,
    }
}

fn utf16_len(bytes: &[u8]) -> usize {
    let mut i = 0usize;
    let mut n = 0usize;
    while i < bytes.len() {
        let Some(seq) = utf8_byte_sequence_length(bytes[i]) else {
            i += 1;
            n += 1;
            continue;
        };
        if i + seq > bytes.len() {
            break;
        }
        let Some(cp) = utf8_decode(&bytes[i..i + seq]) else {
            i += 1;
            n += 1;
            continue;
        };
        n += if cp >= 0x10000 { 2 } else { 1 };
        i += seq;
    }
    n
}

fn position_at(source: &[u8], at: usize) -> Position {
    let mut line = 0usize;
    let mut line_start = 0usize;
    for (i, &c) in source[..at].iter().enumerate() {
        if c == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    Position { line, character: utf16_len(&source[line_start..at]) }
}

/// A range covering the offending token. When the diagnostic carries an
/// exact end (`end_off`, e.g. a `: type` value's span) the squiggle stops
/// there — so a trailing comment stays un-squiggled; otherwise it runs to
/// end-of-line. Mirrors `Diagnostic.locate`'s "resting past a newline means
/// the previous line" backtrack so the squiggle lands on the line the author
/// sees.
fn error_range(source: &[u8], offset: usize, end_off: Option<usize>) -> Range {
    let mut at = offset.min(source.len());
    if at > 0 && source[at - 1] == b'\n' {
        at -= 1;
    }
    let line_end = source[at..].iter().position(|&c| c == b'\n').map_or(source.len(), |p| at + p);
    // Clamp a known token-end to this line and never before the caret.
    let stop = match end_off {
        Some(e) => at.max(e.min(line_end)),
        None => line_end,
    };
    let mut start = position_at(source, at);
    let end = position_at(source, stop);
    // Guarantee a non-empty range so every client renders something.
    if start.line == end.line && start.character == end.character && start.character > 0 {
        start.character -= 1;
    }
    Range { start, end }
}

// ─── tiny JSON writers (output only; input uses fig_core::wire::json) ───

/// One LSP Diagnostic object: range (via `error_range`), severity (1 Error /
/// 2 Warning), code, and the parser's teaching message.
fn write_diagnostic(w: &mut Vec<u8>, source: &[u8], offset: usize, end_off: Option<usize>, severity: u8, code: &str, message: &str) {
    w.extend_from_slice(b"{\"range\":");
    write_range(w, error_range(source, offset, end_off));
    w.extend_from_slice(format!(",\"severity\":{severity},\"source\":\"fig\",\"code\":").as_bytes());
    write_json_string(w, code.as_bytes());
    w.extend_from_slice(b",\"message\":");
    write_json_string(w, message.as_bytes());
    w.push(b'}');
}

fn write_range(w: &mut Vec<u8>, rng: Range) {
    w.extend_from_slice(
        format!(
            "{{\"start\":{{\"line\":{},\"character\":{}}},\"end\":{{\"line\":{},\"character\":{}}}}}",
            rng.start.line, rng.start.character, rng.end.line, rng.end.character
        )
        .as_bytes(),
    );
}

fn write_id(w: &mut Vec<u8>, id: &Value) {
    match id {
        Value::Integer(n) => w.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => write_json_string(w, s.as_bytes()),
        _ => w.extend_from_slice(b"null"),
    }
}

fn write_json_string(w: &mut Vec<u8>, s: &[u8]) {
    w.push(b'"');
    for &c in s {
        match c {
            b'"' => w.extend_from_slice(b"\\\""),
            b'\\' => w.extend_from_slice(b"\\\\"),
            b'\n' => w.extend_from_slice(b"\\n"),
            b'\r' => w.extend_from_slice(b"\\r"),
            b'\t' => w.extend_from_slice(b"\\t"),
            0x08 => w.extend_from_slice(b"\\b"),
            0x0c => w.extend_from_slice(b"\\f"),
            c if c < 0x20 => w.extend_from_slice(format!("\\u{c:04x}").as_bytes()),
            c => w.push(c),
        }
    }
    w.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report with one error and one warning, whatever the text.
    fn fake(_text: &[u8]) -> Report {
        Report {
            diag: Some(Diag { offset: 4, end: None, code: "FigBadKey".into(), message: "bad \"key\"" }),
            errors: vec![
                Diag { offset: 4, end: None, code: "FigBadKey".into(), message: "bad \"key\"" },
                Diag { offset: 10, end: Some(12), code: "FigRootMarker".into(), message: "root" },
            ],
            warnings: vec![Diag { offset: 0, end: Some(1), code: "string_leading_zero".into(), message: "zero" }],
        }
    }

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    /// Run a session of `bodies` through a server, returning what it wrote.
    fn session(bodies: &[&str], diagnose: fn(&[u8]) -> Report) -> String {
        let input: String = bodies.iter().map(|b| frame(b)).collect();
        let mut r = io::Cursor::new(input.into_bytes());
        let mut out = Vec::new();
        let mut server = Server { w: &mut out, docs: HashMap::new(), diagnose };
        while let Ok(Some(body)) = read_message(&mut r) {
            let Ok(v) = json::parse(&body) else { continue };
            if server.handle(&v).unwrap() == Flow::Exit {
                break;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn the_lifecycle_answers_as_the_zig_server_does() {
        let out = session(
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
                r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
                r#"{"jsonrpc":"2.0","id":"x","method":"textDocument/hover","params":{}}"#,
                "not json",
                r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
                r#"{"jsonrpc":"2.0","method":"exit"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#,
            ],
            fake,
        );
        let want = [
            r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"textDocumentSync":1},"serverInfo":{"name":"fig-lsp","version":"0.0.1"}}}"#,
            r#"{"jsonrpc":"2.0","id":"x","result":null}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":null}"#,
        ]
        .map(frame)
        .concat();
        assert_eq!(want, out);
    }

    #[test]
    fn diagnostics_publish_every_error_then_the_warnings_and_a_close_clears_them() {
        let out = session(
            &[
                r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///a.fig","text":"a = 1\nb\n> c = d\n"}}}"#,
                r#"{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///a.fig"}}}"#,
            ],
            fake,
        );
        let want = [
            concat!(
                r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///a.fig","diagnostics":["#,
                r#"{"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":5}},"severity":1,"source":"fig","code":"FigBadKey","message":"bad \"key\""},"#,
                r#"{"range":{"start":{"line":2,"character":2},"end":{"line":2,"character":4}},"severity":1,"source":"fig","code":"FigRootMarker","message":"root"},"#,
                r#"{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"severity":2,"source":"fig","code":"string_leading_zero","message":"zero"}"#,
                r#"]}}"#
            ),
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///a.fig","diagnostics":[]}}"#,
        ]
        .map(frame)
        .concat();
        assert_eq!(want, out);
    }

    #[test]
    fn a_change_publishes_the_last_full_text() {
        fn at_len(text: &[u8]) -> Report {
            Report { diag: Some(Diag { offset: text.len(), end: None, code: "E".into(), message: "m" }), ..Default::default() }
        }
        let out = session(
            &[
                r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"u"},"contentChanges":[{"text":"x"},{"text":"ab\n"}]}}"#,
                r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"u"},"contentChanges":[]}}"#,
            ],
            at_len,
        );
        // At the end, past a newline: the squiggle is on the line before,
        // backed up one so it is not empty.
        assert_eq!(
            frame(r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"u","diagnostics":[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},"severity":1,"source":"fig","code":"E","message":"m"}]}}"#),
            out
        );
    }

    #[test]
    fn framing_reads_any_case_header_and_stops_at_a_clean_eof() {
        let mut r = io::Cursor::new(b"content-LENGTH:  2\r\nContent-Type: x\r\n\r\n{}".to_vec());
        assert_eq!(Some(b"{}".to_vec()), read_message(&mut r).unwrap());
        assert_eq!(None, read_message(&mut r).unwrap());
        let mut r = io::Cursor::new(b"Content-Length: 5\r\n\r\n{}".to_vec());
        assert!(read_message(&mut r).is_err());
        let mut r = io::Cursor::new(b"X: 1\r\n\r\n".to_vec());
        assert!(read_message(&mut r).is_err());
        assert_eq!(Some(1_000), parse_usize("1_000"));
        assert_eq!(None, parse_usize("1__0"));
    }

    #[test]
    fn positions_count_utf16_units_and_ranges_are_never_empty() {
        assert_eq!(0, utf16_len(b""));
        assert_eq!(3, utf16_len("é😀".as_bytes()));
        assert_eq!(2, utf16_len(b"\xff\xfe"));
        assert_eq!(Position { line: 1, character: 2 }, position_at("a\n😀b".as_bytes(), 6));
        // To end of line, from the caret.
        let r = error_range(b"key = value\nnext\n", 6, None);
        assert_eq!((0, 6, 0, 11), (r.start.line, r.start.character, r.end.line, r.end.character));
        // A known end, clamped to the line.
        let r = error_range(b"key = value\nnext\n", 6, Some(100));
        assert_eq!((0, 11), (r.end.line, r.end.character));
        // An empty range backs its start up one.
        let r = error_range(b"ab", 2, None);
        assert_eq!((0, 1, 0, 2), (r.start.line, r.start.character, r.end.line, r.end.character));
        let r = error_range(b"", 0, None);
        assert_eq!((0, 0, 0, 0), (r.start.line, r.start.character, r.end.line, r.end.character));
    }

    #[test]
    fn json_strings_and_tag_names_are_spelled_as_zig_spells_them() {
        let mut w = Vec::new();
        write_json_string(&mut w, b"a\"\\\n\r\t\x08\x0c\x01\x7f");
        assert_eq!(&b"\"a\\\"\\\\\\n\\r\\t\\b\\f\\u0001\x7f\""[..], &w[..]);
        assert_eq!("string_looks_like_literal", tag_name("StringLooksLikeLiteral"));
        let mut w = Vec::new();
        write_id(&mut w, &Value::NumberString("1.5".into()));
        assert_eq!(b"null", &w[..]);
    }
}
