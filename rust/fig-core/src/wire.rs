//! Port of src/contract/wire.zig: the helper wire as a `Language` over any
//! transport.
//!
//! The out-of-process carrier of the runtime-language contract is JSON-RPC
//! 2.0, one message per line — `describe` once, then `parse`, `print`,
//! `render` and `split_stream` per call, one request to one response —
//! specified in `docs/helper-protocol.md`. This module is fig's half of it:
//! a `Language` (`WireLanguage`) whose methods write a request, hand it to a
//! `Transport`, and read the response back into the table and fragments the
//! trait returns. In Zig the envelope is zigjr's and the methods fill a
//! `Runtime.VTable`; here the envelope is written by hand, byte for byte as
//! zigjr writes it, and the JSON is read by the small reader in `json`
//! below, which reads an integer as an integer and keeps any other number as
//! its text, as zigjr's fork does. What carries the line is the transport's
//! business, and there are two: a child process's stdin and stdout
//! (`Process`, the transport of the CLI's helper runner,
//! `src/cli/languages.zig`), and the wasm module's host call (`crate::host`,
//! `src/contract/host.zig`). Both register through the same
//! `Registry::register_arc` a host's own language does (`register`); there
//! is no second way in.
//!
//! `table_to_json` and `table_from_json` are the node table on the wire, and
//! are also what `fig lang table` prints and `fig lang check` holds a twin to
//! — the shape a helper author reads is the shape this module writes.
//!
//! The Zig module's two layers — the vtable thunks (`parseThunk`,
//! `printThunk`, `renderCall`, `splitStreamThunk`, which fill a C
//! `ErrorInfo`) and the runtime registry's `Language` over them, which turns
//! a status into an error name — are kept: the thunks are `WireLanguage`'s
//! `parse_thunk`, `print_view`, `render_call` and `split_stream_thunk`,
//! returning an `ErrorInfo`, and the trait methods are the runtime half.
//!
//! # Public API
//!
//! ```text
//! // Where a request line goes and a response line comes from.
//! pub trait Transport: Send { fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>>; }
//! pub struct Channel { pub transport: Box<dyn Transport>, pub last_id: i64 }
//! pub fn call(c: &mut Channel, method: &str, params: Option<&[u8]>) -> Result<Answer>;
//! pub fn read_answer(line: &[u8], id: i64) -> Result<Answer>;
//!     // HelperSpokeNoJson, HelperSpeaksOldWire, HelperOutOfStep
//! pub enum Answer { Result(json::Value), Refused(Refused) }
//! pub struct Refused { pub code: i64, pub message: String, pub byte_offset: usize }
//! impl Refused { pub fn text(&self) -> String; }
//! pub fn failure_text(err: Error) -> &'static str;
//!
//! // A helper as a language.
//! pub fn describe(transport: Box<dyn Transport>) -> Result<WireLanguage>;
//!     // HelperRefused (`registry::last_refusal` says why), HelperSpokeNoJson
//! pub struct WireLanguage;   // impl Language; the trait's name() is the helper's name
//! pub fn register(registry: &Registry, lang: WireLanguage, options: RegisterOptions) -> Result<i32>;
//!     // InvalidLanguage, HarnessFailed, NameTaken, RegistryFull (Registry::register_with's)
//!
//! // A helper process (the CLI's transport).
//! pub struct Process;        // impl Transport
//! impl Process { pub fn spawn(argv: &[String]) -> Result<Process>; }   // HelperRefused
//!
//! // The node table on the wire.
//! pub fn table_from_json(v: &json::Value, out: &mut Table) -> Result<()>;   // MalformedTable
//! pub fn table_to_json(w: &mut Vec<u8>, v: &table::View<'_>);
//! pub fn json_string(w: &mut Vec<u8>, s: &[u8]);
//! pub struct PrintOptions { pretty, strip_comments, indent, width, splice, flow }  // the C subset
//! pub struct ErrorInfo { pub message: Vec<u8>, pub byte_offset: usize, pub code: i32 }
//! pub mod json { pub enum Value; pub struct Object; pub fn parse(input: &[u8]) -> Result<Value>; }
//! ```
//!
//! **What the CLI's helper runner calls** (`src/cli/languages.zig`'s
//! `register`, given a configured `name` and `command`):
//!
//! ```text
//! let argv = expand_argv(command);                  // the CLI's own `~` expansion
//! let process = wire::Process::spawn(&argv)?;       // refuses "could not start `argv0`: …"
//! let lang = wire::describe(Box::new(process))?;    // HelperRefused / HelperSpokeNoJson
//! if lang.name() != name {                          // the CLI's check, its own refusal
//!     registry::refuse(&format!("the helper describes itself as `{}`, but languages.figl calls it `{name}`", lang.name()));
//!     return Err(err!(HelperNameMismatch));
//! }
//! let abi = wire::register(registry::global(), lang, RegisterOptions::default())?;
//! registry::global().entry_by_abi(abi)
//! ```
//!
//! and `fig lang table` prints a table with `table_to_json` over
//! `convert::full_table(&doc).view()`. The wasm CLI's host languages
//! (`cli/host_languages.zig`) go through `crate::host::register`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::err;
use crate::error::Error;
use crate::kit::ast::{ExtKind, NodeId, SerializeOptions, AST};
use crate::kit::convert;
use crate::kit::document::MentionKind;
use crate::kit::json_string;
use crate::kit::language::{self as language, Language};
use crate::kit::manifest::{
    Caps, ClosedContainers, CommentDelimiter, Comments, Delimiters, Dialect, DialectInfo, Literal, NativeKinds, Prints,
    PrintsComments, RenderRequest, Renderer, SectionHeader, SpliceStyle, StreamDocument, Syntax,
};
use crate::kit::table::{self, Table, NONE};
use crate::registry::{self, RegisterOptions, Registry};
use crate::Result;

use json::{Object, Value};

// ============================================================================
// JSON
// ============================================================================

/// The JSON the wire reads: a tree, read strictly (RFC 8259), with every
/// integer lexeme that fits an `i64` an `Integer` and any other number kept
/// as its text (`NumberString`) — the wire carries no floats, and zigjr's
/// fork reads none (`wire_value.zig`). Objects keep their keys in order; a
/// repeated key takes the later value in the earlier place, as
/// `std.json.ObjectMap.put` does.
pub mod json {
    use crate::err;
    use crate::Result;

    /// One JSON value (Zig's `std.json.Value`, less `float`).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Value {
        Null,
        Bool(bool),
        Integer(i64),
        /// A number that is not an `i64`: a fraction, an exponent, or an
        /// integer too large — as written.
        NumberString(String),
        String(String),
        Array(Vec<Value>),
        Object(Object),
    }

    /// A JSON object's members, in order.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct Object {
        pub entries: Vec<(String, Value)>,
    }

    impl Object {
        pub fn get(&self, key: &str) -> Option<&Value> {
            self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        }

        /// Set `key`, in place when it is there already.
        pub fn put(&mut self, key: String, value: Value) {
            match self.entries.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => self.entries.push((key, value)),
            }
        }

        pub fn len(&self) -> usize {
            self.entries.len()
        }

        pub fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }
    }

    impl Value {
        pub fn as_str(&self) -> Option<&str> {
            match self {
                Value::String(s) => Some(s),
                _ => None,
            }
        }
        pub fn as_object(&self) -> Option<&Object> {
            match self {
                Value::Object(o) => Some(o),
                _ => None,
            }
        }
        pub fn as_array(&self) -> Option<&[Value]> {
            match self {
                Value::Array(a) => Some(a),
                _ => None,
            }
        }
        pub fn as_i64(&self) -> Option<i64> {
            match self {
                Value::Integer(i) => Some(*i),
                _ => None,
            }
        }
        pub fn as_bool(&self) -> Option<bool> {
            match self {
                Value::Bool(b) => Some(*b),
                _ => None,
            }
        }
        /// `self[key]`, for an object.
        pub fn get(&self, key: &str) -> Option<&Value> {
            self.as_object()?.get(key)
        }
    }

    /// Deeper than this is refused within one message's value, so a hostile
    /// message cannot exhaust the stack (zigjr's `max_depth`).
    pub const MAX_DEPTH: usize = 128;

    /// How deep a document `parse` reads: `std.json` has no bound, but a
    /// recursive reader needs one.
    const PARSE_DEPTH: usize = 4096;

    /// A whole document: one value, whitespace around it, nothing else.
    /// Fails with `SyntaxError`.
    pub fn parse(input: &[u8]) -> Result<Value> {
        let mut s = Scanner::new(input);
        let v = s.value(0, PARSE_DEPTH)?;
        s.end()?;
        Ok(v)
    }

    /// A cursor over complete input (`std.json.Scanner.initCompleteInput`).
    pub struct Scanner<'a> {
        s: &'a [u8],
        pub i: usize,
    }

    impl<'a> Scanner<'a> {
        pub fn new(s: &'a [u8]) -> Scanner<'a> {
            Scanner { s, i: 0 }
        }

        pub fn skip_ws(&mut self) {
            while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            }
        }

        /// The next significant byte, not taken.
        pub fn peek(&mut self) -> Option<u8> {
            self.skip_ws();
            self.s.get(self.i).copied()
        }

        /// Take `c`, the next significant byte, or fail.
        pub fn expect(&mut self, c: u8) -> Result<()> {
            if self.peek() != Some(c) {
                return Err(err!(SyntaxError));
            }
            self.i += 1;
            Ok(())
        }

        /// Nothing but whitespace is left.
        pub fn end(&mut self) -> Result<()> {
            if self.peek().is_some() {
                return Err(err!(SyntaxError));
            }
            Ok(())
        }

        /// The next key of an object whose `{` has been read, its `:` taken,
        /// or `None` at its `}` (zigjr's `nextKey`). `first` is whether no
        /// member has been read yet.
        pub fn next_key(&mut self, first: bool) -> Result<Option<String>> {
            if self.peek() == Some(b'}') {
                self.i += 1;
                return Ok(None);
            }
            if !first {
                self.expect(b',')?;
            }
            if self.peek() != Some(b'"') {
                return Err(err!(SyntaxError));
            }
            let k = self.string()?;
            self.expect(b':')?;
            Ok(Some(k))
        }

        /// One value, at most `max - depth` levels deep.
        pub fn value(&mut self, depth: usize, max: usize) -> Result<Value> {
            if depth > max {
                return Err(err!(SyntaxError));
            }
            match self.peek() {
                None => Err(err!(SyntaxError)),
                Some(b'n') => self.word(b"null", Value::Null),
                Some(b't') => self.word(b"true", Value::Bool(true)),
                Some(b'f') => self.word(b"false", Value::Bool(false)),
                Some(b'"') => Ok(Value::String(self.string()?)),
                Some(b'[') => {
                    self.i += 1;
                    let mut items = Vec::new();
                    if self.peek() == Some(b']') {
                        self.i += 1;
                        return Ok(Value::Array(items));
                    }
                    loop {
                        items.push(self.value(depth + 1, max)?);
                        match self.peek() {
                            Some(b',') => self.i += 1,
                            Some(b']') => {
                                self.i += 1;
                                return Ok(Value::Array(items));
                            }
                            _ => return Err(err!(SyntaxError)),
                        }
                    }
                }
                Some(b'{') => {
                    self.i += 1;
                    let mut o = Object::default();
                    let mut first = true;
                    while let Some(k) = self.next_key(first)? {
                        first = false;
                        let v = self.value(depth + 1, max)?;
                        o.put(k, v);
                    }
                    Ok(Value::Object(o))
                }
                Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
                Some(_) => Err(err!(SyntaxError)),
            }
        }

        fn word(&mut self, w: &[u8], v: Value) -> Result<Value> {
            if self.s[self.i..].starts_with(w) {
                self.i += w.len();
                // A literal runs into nothing that could continue a token.
                if let Some(&c) = self.s.get(self.i) {
                    if c.is_ascii_alphanumeric() {
                        return Err(err!(SyntaxError));
                    }
                }
                Ok(v)
            } else {
                Err(err!(SyntaxError))
            }
        }

        fn digits(&mut self) -> usize {
            let from = self.i;
            while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                self.i += 1;
            }
            self.i - from
        }

        fn number(&mut self) -> Result<Value> {
            let start = self.i;
            if self.s[self.i] == b'-' {
                self.i += 1;
            }
            match self.s.get(self.i) {
                Some(b'0') => self.i += 1,
                Some(c) if c.is_ascii_digit() => {
                    self.digits();
                }
                _ => return Err(err!(SyntaxError)),
            }
            if self.s.get(self.i) == Some(&b'.') {
                self.i += 1;
                if self.digits() == 0 {
                    return Err(err!(SyntaxError));
                }
            }
            if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
                self.i += 1;
                if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                    self.i += 1;
                }
                if self.digits() == 0 {
                    return Err(err!(SyntaxError));
                }
            }
            if let Some(&c) = self.s.get(self.i) {
                if c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'+' {
                    return Err(err!(SyntaxError));
                }
            }
            // Valid UTF-8: the lexeme is ASCII.
            let lexeme = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| err!(SyntaxError))?;
            Ok(match lexeme.parse::<i64>() {
                Ok(i) => Value::Integer(i),
                Err(_) => Value::NumberString(lexeme.to_string()),
            })
        }

        fn hex4(&mut self) -> Result<u32> {
            let h = self.s.get(self.i..self.i + 4).ok_or(err!(SyntaxError))?;
            let mut v = 0u32;
            for &c in h {
                let d = (c as char).to_digit(16).ok_or(err!(SyntaxError))?;
                v = v * 16 + d;
            }
            self.i += 4;
            Ok(v)
        }

        /// A string, its `"` next; decoded. Its bytes must be UTF-8, with no
        /// raw control character, and a `\u` escape of a surrogate must be
        /// half of a pair.
        pub fn string(&mut self) -> Result<String> {
            self.expect(b'"')?;
            let mut out: Vec<u8> = Vec::new();
            loop {
                let Some(&c) = self.s.get(self.i) else { return Err(err!(SyntaxError)) };
                match c {
                    b'"' => {
                        self.i += 1;
                        return String::from_utf8(out).map_err(|_| err!(SyntaxError));
                    }
                    b'\\' => {
                        self.i += 1;
                        let Some(&e) = self.s.get(self.i) else { return Err(err!(SyntaxError)) };
                        self.i += 1;
                        match e {
                            b'"' => out.push(b'"'),
                            b'\\' => out.push(b'\\'),
                            b'/' => out.push(b'/'),
                            b'b' => out.push(0x08),
                            b'f' => out.push(0x0c),
                            b'n' => out.push(b'\n'),
                            b'r' => out.push(b'\r'),
                            b't' => out.push(b'\t'),
                            b'u' => {
                                let mut cp = self.hex4()?;
                                if (0xD800..0xDC00).contains(&cp) {
                                    if !self.s[self.i..].starts_with(b"\\u") {
                                        return Err(err!(SyntaxError));
                                    }
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&lo) {
                                        return Err(err!(SyntaxError));
                                    }
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                } else if (0xDC00..0xE000).contains(&cp) {
                                    return Err(err!(SyntaxError));
                                }
                                let ch = char::from_u32(cp).ok_or(err!(SyntaxError))?;
                                let mut buf = [0u8; 4];
                                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                            _ => return Err(err!(SyntaxError)),
                        }
                    }
                    0x00..=0x1f => return Err(err!(SyntaxError)),
                    _ => {
                        out.push(c);
                        self.i += 1;
                    }
                }
            }
        }
    }
}

/// Write a JSON string literal (`std.json.Stringify.encodeJsonString`, which
/// escapes exactly what the kit's `json_string` does).
pub fn json_string(w: &mut Vec<u8>, s: &[u8]) {
    json_string::write_quoted(w, s);
}

// ============================================================================
// THE ENVELOPE
// ============================================================================

/// Where a request line goes and a response line comes from: the CLI's
/// helper process (`Process`), the wasm module's host call
/// (`crate::host`). Each request takes the next id (`Channel::last_id`).
pub trait Transport: Send {
    /// Send one request line and take one response line. Neither line
    /// carries its newline. An error is reported by name to whoever asked,
    /// so the set is the transport's own; `HelperExited` is the one every
    /// transport can raise.
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>>;
}

/// A transport and the id the last request carried (Zig's `Transport`, whose
/// `last_id` is a field of its own): each request takes the next, and its
/// response must carry the same.
pub struct Channel {
    pub transport: Box<dyn Transport>,
    pub last_id: i64,
}

impl Channel {
    pub fn new(transport: Box<dyn Transport>) -> Channel {
        Channel { transport, last_id: 0 }
    }
}

/// What a helper answered a request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The method's result. A number that is not an integer is its text
    /// (`NumberString`): the wire carries no floats.
    Result(Value),
    /// The helper's error object.
    Refused(Refused),
}

/// A JSON-RPC error: `code` as `docs/helper-protocol.md` assigns them,
/// `message` for a person, and `byte_offset` from `data` where the helper
/// gave one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Refused {
    pub code: i64,
    pub message: String,
    pub byte_offset: usize,
}

impl Refused {
    /// The message to report: the helper's own, or, for the codes JSON-RPC
    /// reserves for a request the helper could not read, that said first.
    pub fn text(&self) -> String {
        if self.code >= -32768 && self.code <= -32100 {
            return format!("the helper could not handle fig's request (JSON-RPC error {}): {}", self.code, self.message);
        }
        self.message.clone()
    }
}

/// The ways an answer can fail to be one, beyond the transport's own:
/// `HelperSpokeNoJson` (the line is not a JSON-RPC response object),
/// `HelperSpeaksOldWire` (a JSON object without `"jsonrpc":"2.0"`: a helper
/// built against fig 5, whose wire was `{"op":…}` / `{"ok":…}`),
/// `HelperOutOfStep` (the response's id is not the request's).
pub const ANSWER_ERRORS: &[&str] = &["HelperSpokeNoJson", "HelperSpeaksOldWire", "HelperOutOfStep"];

/// zigjr's `writeRequestJson`, for a numeric id: its spacing is the bytes on
/// the wire.
fn write_request(out: &mut Vec<u8>, method: &str, params: Option<&[u8]>, id: i64) {
    out.extend_from_slice(b"{\"jsonrpc\": \"2.0\", \"method\": \"");
    out.extend_from_slice(method.as_bytes());
    out.extend_from_slice(b"\"");
    if let Some(p) = params {
        out.extend_from_slice(b", \"params\": ");
        out.extend_from_slice(p);
    }
    out.extend_from_slice(format!(", \"id\": {id}}}").as_bytes());
}

/// Send `method` with `params` (a JSON object's text, or `None` for none)
/// and read the answer.
pub fn call(c: &mut Channel, method: &str, params: Option<&[u8]>) -> Result<Answer> {
    c.last_id += 1;
    let id = c.last_id;
    let mut req = Vec::new();
    write_request(&mut req, method, params, id);
    let line = c.transport.exchange(&req)?;
    read_answer(&line, id)
}

/// The envelope's fields as zigjr's `RpcResponse.jsonParse` reads them.
struct Envelope {
    jsonrpc: [u8; 3],
    /// `None` for a `null` id or none, else the number or string.
    id: Option<std::result::Result<i64, String>>,
    result: Value,
    code: i64,
    message: String,
    data: Option<Value>,
}

fn read_envelope(s: &mut json::Scanner<'_>) -> Result<Envelope> {
    s.expect(b'{')?;
    let mut env = Envelope { jsonrpc: *b"0.0", id: None, result: Value::Null, code: 0, message: String::new(), data: None };
    let mut first = true;
    while let Some(key) = s.next_key(first)? {
        first = false;
        match key.as_str() {
            "jsonrpc" => {
                let Value::String(v) = s.value(0, json::MAX_DEPTH)? else { return Err(err!(UnexpectedToken)) };
                let b = v.as_bytes();
                if b.len() != 3 {
                    return Err(err!(LengthMismatch));
                }
                env.jsonrpc = [b[0], b[1], b[2]];
            }
            "id" => {
                env.id = match s.value(0, json::MAX_DEPTH)? {
                    Value::Null => None,
                    Value::Integer(i) => Some(Ok(i)),
                    Value::String(x) => Some(Err(x)),
                    _ => return Err(err!(UnexpectedToken)),
                };
            }
            "result" => env.result = s.value(0, json::MAX_DEPTH)?,
            "error" => {
                // `RpcResponseError.jsonParse`: an object, field by field;
                // a repeated `error` starts from its defaults again.
                s.expect(b'{')?;
                env.code = 0;
                env.message.clear();
                env.data = None;
                let mut first = true;
                while let Some(k) = s.next_key(first)? {
                    first = false;
                    match k.as_str() {
                        "code" => {
                            env.code = match s.value(0, json::MAX_DEPTH)? {
                                Value::Integer(i) if i >= i32::MIN as i64 && i <= i32::MAX as i64 => i,
                                _ => return Err(err!(Overflow)),
                            }
                        }
                        "message" => {
                            let Value::String(m) = s.value(0, json::MAX_DEPTH)? else { return Err(err!(UnexpectedToken)) };
                            env.message = m;
                        }
                        "data" => env.data = Some(s.value(0, json::MAX_DEPTH)?),
                        _ => {
                            s.value(0, usize::MAX)?;
                        }
                    }
                }
            }
            _ => {
                // An unknown field is skipped (`ignore_unknown_fields`).
                s.value(0, usize::MAX)?;
            }
        }
    }
    Ok(env)
}

/// One response line, checked against the request's `id`. Fails with one of
/// `ANSWER_ERRORS`.
pub fn read_answer(line: &[u8], id: i64) -> Result<Answer> {
    let mut s = json::Scanner::new(line);
    if s.peek() != Some(b'{') {
        return Err(err!(HelperSpokeNoJson));
    }
    let Ok(resp) = read_envelope(&mut s) else { return Err(err!(HelperSpokeNoJson)) };
    if s.end().is_err() {
        return Err(err!(HelperSpokeNoJson));
    }
    if &resp.jsonrpc != b"2.0" {
        return Err(err!(HelperSpeaksOldWire));
    }
    if resp.id != Some(Ok(id)) {
        return Err(err!(HelperOutOfStep));
    }
    if resp.code != 0 || !resp.message.is_empty() {
        let mut offset = 0usize;
        if let Some(Value::Object(d)) = &resp.data {
            if let Some(Value::Integer(o)) = d.get("byte_offset") {
                if *o >= 0 {
                    offset = *o as usize;
                }
            }
        }
        return Ok(Answer::Refused(Refused { code: resp.code, message: resp.message, byte_offset: offset }));
    }
    Ok(Answer::Result(resp.result))
}

/// A sentence for a failed call, where the error's name would be a Zig name.
pub fn failure_text(err: Error) -> &'static str {
    match err.name() {
        "HelperSpeaksOldWire" => "the helper speaks fig 5's wire, not JSON-RPC 2.0; rebuild it against fig 6",
        "HelperOutOfStep" => "the helper answered a different request than the one fig sent",
        "HelperSpokeNoJson" => "the helper's answer is not a JSON-RPC response",
        other => other,
    }
}

// ============================================================================
// THE C SHAPES THE THUNKS SPEAK
// ============================================================================

/// The C `ErrorInfo` a thunk fills: a message (at most 255 bytes, as the C
/// buffer holds), where in the input, and a printer's `Refusal` code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorInfo {
    pub message: Vec<u8>,
    pub byte_offset: usize,
    pub code: i32,
}

impl ErrorInfo {
    const CAP: usize = 255;

    fn of(message: &[u8], offset: usize) -> ErrorInfo {
        let n = message.len().min(Self::CAP);
        ErrorInfo { message: message[..n].to_vec(), byte_offset: offset, code: 0 }
    }

    pub fn text(&self) -> &[u8] {
        &self.message
    }
}

/// The subset of `SerializeOptions` a printer outside core is told (the C
/// `PrintOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintOptions {
    pub pretty: bool,
    pub strip_comments: bool,
    pub indent: u8,
    pub width: u16,
    pub splice: bool,
    pub flow: bool,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions { pretty: true, strip_comments: false, indent: 2, width: 80, splice: false, flow: false }
    }
}

impl PrintOptions {
    /// What `printNodeWith` tells a runtime printer of `options`.
    pub fn of(options: &SerializeOptions) -> PrintOptions {
        PrintOptions {
            pretty: options.pretty,
            strip_comments: options.strip_comments,
            indent: options.indent,
            width: options.width,
            splice: options.splice,
            flow: options.flow,
        }
    }
}

/// The C `Refusal` codes a printer names, 1 to 9, as the errors
/// `printNodeWith` returns for them.
const REFUSALS: [Error; 9] = [
    err!(NullUnsupported),
    err!(NonStringKey),
    err!(UnrepresentableRoot),
    err!(UnsupportedValue),
    err!(InvalidKey),
    err!(NonFiniteNumber),
    err!(UnresolvedAlias),
    err!(NestingTooDeep),
    err!(UnexpectedNodeKind),
];

/// A parse that bypasses the wire (`host.zig`'s `nativeParse`): the dialect's
/// name, the input, the table to fill. On a refusal it fills the `ErrorInfo`.
pub(crate) type NativeParse = Box<dyn Fn(&str, &[u8], &mut Table) -> std::result::Result<(), ErrorInfo> + Send + Sync>;
/// A print that bypasses the wire (`host.zig`'s `nativePrint`).
pub(crate) type NativePrint =
    Box<dyn Fn(&str, &table::View<'_>, &PrintOptions) -> std::result::Result<Vec<u8>, ErrorInfo> + Send + Sync>;

// ============================================================================
// DESCRIPTION → LANGUAGE
// ============================================================================

/// A runtime language whose calls go over the wire: built by `describe` from
/// what the helper says of itself, and registered like any language. Its
/// strings live as long as the process, as a registration's do.
pub struct WireLanguage {
    channel: Mutex<Channel>,
    name: &'static str,
    caps: Caps,
    syntax: Option<Syntax>,
    dialects: &'static [DialectInfo],
    /// Each dialect row's own syntax, and its extensions.
    dialect_syntax: Vec<Option<Syntax>>,
    dialect_extensions: Vec<Vec<&'static str>>,
    /// Every row's extensions, in order.
    extensions: &'static [&'static str],
    samples: &'static [&'static [u8]],
    /// `value`, `entry`, `item`, `tail`, `key`, `key_name`, `string`.
    renderers: [bool; 7],
    split_stream: bool,
    /// The C record's rules a `Syntax` cannot state: a half-stated
    /// `closed_containers` or `section_header`, for `register` to refuse.
    record_faults: Vec<&'static str>,
    pub(crate) native_parse: Option<NativeParse>,
    pub(crate) native_print: Option<NativePrint>,
}

impl std::fmt::Debug for WireLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireLanguage").field("name", &self.name).field("caps", &self.caps).finish()
    }
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn leak_bytes(s: &[u8]) -> &'static [u8] {
    Box::leak(s.to_vec().into_boxed_slice())
}

/// A string as the C record holds it: NUL-terminated, so it ends at its
/// first NUL (`dupeSentinel`, read back with `std.mem.span`).
fn zstr(s: &str) -> &str {
    match s.find('\0') {
        Some(n) => &s[..n],
        None => s,
    }
}

fn opt_string(o: &Object, key: &str) -> Option<&'static [u8]> {
    match o.get(key) {
        Some(Value::String(s)) => Some(leak_bytes(zstr(s).as_bytes())),
        _ => None,
    }
}

fn bool_or(o: &Object, key: &str, default: bool) -> bool {
    match o.get(key) {
        Some(Value::Bool(b)) => *b,
        _ => default,
    }
}

/// The byte `o[name]` states as an integer, or `None` — the C record's 0 —
/// when it is absent or not a byte.
fn byte_or_none(o: &Object, name: &str) -> Option<u8> {
    match o.get(name) {
        Some(Value::Integer(i)) if *i > 0 && *i < 256 => Some(*i as u8),
        _ => None,
    }
}

/// A delimiter object as the runtime registry reads its record: none without
/// an `open`, and `close` empty when not stated.
fn delimiter_of(v: Option<&Value>) -> Option<CommentDelimiter> {
    let Some(Value::Object(d)) = v else { return None };
    let open = opt_string(d, "open")?;
    Some(CommentDelimiter { open, close: opt_string(d, "close").unwrap_or(b""), forbidden: opt_string(d, "forbidden") })
}

/// The kinds an object names, each a boolean field (`lossless`, `holds`).
fn native_kinds_of(o: &Object) -> NativeKinds {
    let mut kinds = NativeKinds::NONE;
    kinds.null = bool_or(o, "null", false);
    for k in ExtKind::ALL {
        kinds.set_ext(k, bool_or(o, k.name(), false));
    }
    kinds
}

/// A dialect's `prints` object, or `None` for none: `holds` an object of the
/// kinds held (as `lossless` is), `typed_scalars`, `null_at_root` and
/// `root_container` booleans, `char_as` and `number_special_as` strings, and
/// `comments` one of "always", "pretty" or "never".
fn prints_of_wire(v: Option<&Value>) -> Option<Prints> {
    let Some(Value::Object(o)) = v else { return None };
    let holds = match o.get("holds") {
        Some(Value::Object(h)) => native_kinds_of(h),
        _ => NativeKinds::NONE,
    };
    let comments = match o.get("comments") {
        Some(Value::String(c)) if c == "pretty" => PrintsComments::Pretty,
        Some(Value::String(c)) if c == "never" => PrintsComments::Never,
        _ => PrintsComments::Always,
    };
    Some(Prints {
        holds,
        typed_scalars: bool_or(o, "typed_scalars", true),
        null_at_root: bool_or(o, "null_at_root", false),
        root_container: bool_or(o, "root_container", false),
        char_as: opt_string(o, "char_as").map_or("string", |s| leak_str(std::str::from_utf8(s).unwrap_or("string"))),
        number_special_as: opt_string(o, "number_special_as")
            .map_or("string", |s| leak_str(std::str::from_utf8(s).unwrap_or("string"))),
        comments,
    })
}

/// A `syntax` object as `manifest::Syntax`: the wire's `syntaxOf` into the C
/// record and the runtime registry's `syntaxOf` out of it, every field not
/// stated its Zig default. A half-stated `closed_containers` or
/// `section_header` is noted in `faults`, which the C record's validation
/// refuses.
fn syntax_of(o: &Object, faults: &mut Vec<&'static str>) -> Syntax {
    let mut comments = Comments::NONE;
    if let Some(Value::Object(c)) = o.get("comments") {
        comments.line = delimiter_of(c.get("line"));
        comments.trailing = delimiter_of(c.get("trailing"));
        comments.block = delimiter_of(c.get("block"));
    }
    let defaults = Syntax::new(Comments::NONE, None, None);
    let mut s = Syntax {
        comments,
        kv_sep: opt_string(o, "kv_sep"),
        flow_kv_sep_from_siblings: bool_or(o, "flow_kv_sep_from_siblings", false),
        flow_map_pad: opt_string(o, "flow_map_pad").unwrap_or(defaults.flow_map_pad),
        key_sigil: byte_or_none(o, "key_sigil"),
        empty_map_literal: opt_string(o, "empty_map_literal"),
        block_seq_editable: bool_or(o, "block_seq_editable", true),
        flow_containers: bool_or(o, "flow_containers", true),
        indent_unit: opt_string(o, "indent_unit").unwrap_or(defaults.indent_unit),
        seq_item_marker: opt_string(o, "seq_item_marker").unwrap_or(defaults.seq_item_marker),
        closed_containers: None,
        single_line_block_mapping: bool_or(o, "single_line_block_mapping", false),
        bare_document_mapping: bool_or(o, "bare_document_mapping", true),
        flow_map_open: opt_string(o, "flow_map_open").unwrap_or(defaults.flow_map_open),
        flow_map_close: opt_string(o, "flow_map_close").unwrap_or(defaults.flow_map_close),
        structural_indent: bool_or(o, "structural_indent", false),
        section_noun: opt_string(o, "section_noun"),
        section_header: None,
        merge_key: opt_string(o, "merge_key"),
        flow_entry_sep: opt_string(o, "flow_entry_sep").unwrap_or(defaults.flow_entry_sep),
        flow_root: bool_or(o, "flow_root", false),
        flow_maps_only: bool_or(o, "flow_maps_only", false),
        line_prefix_marks: opt_string(o, "line_prefix_marks").unwrap_or(defaults.line_prefix_marks),
        flow_map_kv_sep: opt_string(o, "flow_map_kv_sep").unwrap_or(defaults.flow_map_kv_sep),
        flow_map_entry_sep: opt_string(o, "flow_map_entry_sep").unwrap_or(defaults.flow_map_entry_sep),
        line_continuation: byte_or_none(o, "line_continuation"),
        empty_seq_literal: opt_string(o, "empty_seq_literal"),
        opener_line_comment: bool_or(o, "opener_line_comment", false),
    };
    if let Some(Value::Object(c)) = o.get("closed_containers") {
        if let Some(map_open) = opt_string(c, "map_open") {
            match (opt_string(c, "map_close"), opt_string(c, "seq_open"), opt_string(c, "seq_close")) {
                (Some(map_close), Some(seq_open), Some(seq_close)) => {
                    s.closed_containers = Some(ClosedContainers {
                        map: Delimiters { open: map_open, close: map_close },
                        seq: Delimiters { open: seq_open, close: seq_close },
                    });
                }
                _ => faults.push("closed_containers needs all four tokens"),
            }
        }
    }
    if let Some(Value::Object(h)) = o.get("section_header") {
        if let Some(open) = opt_string(h, "open") {
            match opt_string(h, "close") {
                Some(close) => {
                    s.section_header = Some(SectionHeader {
                        open,
                        close,
                        seq_open: opt_string(h, "seq_open"),
                        seq_close: opt_string(h, "seq_close"),
                        sep: opt_string(h, "sep").unwrap_or(b"."),
                        skip_index: bool_or(h, "skip_index", true),
                    });
                }
                None => faults.push("section_header needs a close token"),
            }
        }
    }
    s
}

const RENDERER_NAMES: [&str; 7] = ["value", "entry", "item", "tail", "key", "key_name", "string"];

fn renderer_index(which: Renderer) -> usize {
    match which {
        Renderer::Value => 0,
        Renderer::Entry => 1,
        Renderer::Item => 2,
        Renderer::Tail => 3,
        Renderer::Key => 4,
        Renderer::KeyName => 5,
        Renderer::String => 6,
    }
}

/// Ask the transport to `describe` itself and build the language that
/// description declares. The caller registers the result (`register`); the
/// CLI checks the name against its configuration first.
///
/// Errors: `HelperRefused` (the transport failed, or the helper answered
/// `describe` with a refusal; `registry::last_refusal` has the reason),
/// `HelperSpokeNoJson` (the description is not the shape the wire
/// documents).
pub fn describe(transport: Box<dyn Transport>) -> Result<WireLanguage> {
    let mut channel = Channel::new(transport);
    let answer = match call(&mut channel, "describe", None) {
        Ok(a) => a,
        Err(e) => {
            registry::refuse(&format!("the helper did not describe itself: {}", failure_text(e)));
            return Err(err!(HelperRefused));
        }
    };
    let desc = match answer {
        Answer::Result(r) => r,
        Answer::Refused(r) => {
            registry::refuse(&format!("the helper refused to describe itself: {}", r.text()));
            return Err(err!(HelperRefused));
        }
    };
    language_from_description(channel, &desc)
}

fn language_from_description(channel: Channel, desc: &Value) -> Result<WireLanguage> {
    let Value::Object(o) = desc else { return Err(err!(HelperSpokeNoJson)) };
    let Some(Value::String(name)) = o.get("name") else { return Err(err!(HelperSpokeNoJson)) };

    let mut caps = Caps { read: false, ..Caps::DEFAULT };
    if let Some(Value::Object(c)) = o.get("caps") {
        caps.read = bool_or(c, "read", false);
        caps.edit = bool_or(c, "edit", false);
        caps.serialize = bool_or(c, "serialize", false);
        caps.references = bool_or(c, "references", false);
    }
    // `null` (or absent) is unbounded; 0 is a flat format.
    caps.max_mapping_depth = match o.get("max_mapping_depth") {
        Some(Value::Integer(d)) if *d >= 0 && *d < 256 => Some(*d as u8),
        _ => None,
    };
    // An object is the envelope, with the kinds it names held natively.
    caps.lossless = match o.get("lossless") {
        Some(Value::Object(l)) => Some(native_kinds_of(l)),
        _ => None,
    };

    let mut faults: Vec<&'static str> = Vec::new();
    let syntax = match o.get("syntax") {
        Some(Value::Object(s)) => Some(syntax_of(s, &mut faults)),
        _ => None,
    };

    let mut dialects: Vec<DialectInfo> = Vec::new();
    let mut dialect_syntax: Vec<Option<Syntax>> = Vec::new();
    let mut dialect_extensions: Vec<Vec<&'static str>> = Vec::new();
    let mut extensions: Vec<&'static str> = Vec::new();
    if let Some(Value::Array(dv)) = o.get("dialects") {
        for d in dv {
            let Value::Object(d) = d else { continue };
            let Some(Value::String(dn)) = d.get("name") else { continue };
            let mut exts: Vec<&'static str> = Vec::new();
            if let Some(Value::Array(ev)) = d.get("extensions") {
                for x in ev {
                    if let Value::String(x) = x {
                        exts.push(leak_str(zstr(x)));
                    }
                }
            }
            let splice = match d.get("splice") {
                Some(Value::String(sp)) if sp == "escaped" => SpliceStyle::Escaped,
                Some(Value::String(sp)) if sp == "raw" => SpliceStyle::Raw,
                _ => SpliceStyle::Literal,
            };
            let index = dialects.len();
            dialects.push(DialectInfo {
                dialect: index as Dialect,
                prints: prints_of_wire(d.get("prints")),
                ..DialectInfo::new(leak_str(zstr(dn)), splice, opt_string(d, "empty_doc_seed"))
            });
            dialect_syntax.push(match d.get("syntax") {
                Some(Value::Object(s)) => Some(syntax_of(s, &mut faults)),
                _ => None,
            });
            extensions.extend(exts.iter().copied());
            dialect_extensions.push(exts);
        }
    }

    let mut samples: Vec<&'static [u8]> = Vec::new();
    if let Some(Value::Array(sv)) = o.get("samples") {
        for x in sv {
            if let Value::String(x) = x {
                samples.push(leak_bytes(x.as_bytes()));
            }
        }
    }

    let mut renderers = [false; 7];
    if let Some(Value::Array(rv)) = o.get("renderers") {
        for x in rv {
            if let Value::String(x) = x {
                for (i, n) in RENDERER_NAMES.iter().enumerate() {
                    if x == n {
                        renderers[i] = true;
                    }
                }
            }
        }
    }

    Ok(WireLanguage {
        channel: Mutex::new(channel),
        name: leak_str(zstr(name)),
        caps,
        syntax,
        dialects: Box::leak(dialects.into_boxed_slice()),
        dialect_syntax,
        dialect_extensions,
        extensions: Box::leak(extensions.into_boxed_slice()),
        samples: Box::leak(samples.into_boxed_slice()),
        renderers,
        split_stream: bool_or(o, "split_stream", false),
        record_faults: faults,
        native_parse: None,
        native_print: None,
    })
}

/// Register `lang` in `registry`, as `Runtime.register` registers the vtable
/// `describe` built: first the C record's rules a `Syntax` cannot carry
/// (`validateVTable`: an editable language must state a syntax, and
/// `closed_containers` and `section_header` must be whole), then
/// `Registry::register_with`, which validates the rest and runs the harness
/// over the samples, calling the helper for each. Returns the ABI integer of
/// the first dialect row.
///
/// Errors: `InvalidLanguage` (`registry::last_refusal` says why),
/// `HarnessFailed`, `NameTaken`, `RegistryFull`.
pub fn register(registry: &Registry, lang: WireLanguage, options: RegisterOptions) -> Result<i32> {
    registry::clear_refusal();
    if let Err(msg) = lang.validate_record() {
        registry::refuse(&msg);
        return Err(err!(InvalidLanguage));
    }
    registry.register_arc(Arc::new(lang), options)
}

impl WireLanguage {
    /// The C record's rules that `registry::validate_language` cannot see
    /// once the description is a `Syntax`.
    fn validate_record(&self) -> std::result::Result<(), String> {
        let name = self.name;
        if name.is_empty() || !self.caps.read || self.dialects.is_empty() {
            // The registry's own rules say these.
            return Ok(());
        }
        if self.caps.edit {
            if self.syntax.is_none() {
                return Err(format!("'{name}' declares the edit capability and must supply syntax"));
            }
            if let Some(f) = self.record_faults.first() {
                return Err(format!("'{name}': {f}"));
            }
        }
        Ok(())
    }

    fn dialect_name(&self, d: Dialect) -> &'static str {
        self.dialects.get(d as usize).map_or(self.name, |r| r.name)
    }

    fn call(&self, method: &str, params: &[u8]) -> Result<Answer> {
        let mut c = self.channel.lock().unwrap_or_else(|p| p.into_inner());
        call(&mut c, method, Some(params))
    }

    // ── the wire, per call (the vtable's thunks) ────────────────────────────

    /// `parse` with `{"dialect","input"}`: the table the helper answers,
    /// decoded into `out`, or the reason it gave.
    pub fn parse_thunk(&self, dialect: &str, input: &[u8], out: &mut Table) -> std::result::Result<(), ErrorInfo> {
        if let Some(native) = &self.native_parse {
            return native(dialect, input, out);
        }
        let Ok(src) = std::str::from_utf8(input) else {
            return Err(ErrorInfo::of(b"the input is not UTF-8, which the helper wire carries as text", 0));
        };
        let mut req = Vec::new();
        req.extend_from_slice(b"{\"dialect\":");
        json_string(&mut req, dialect.as_bytes());
        req.extend_from_slice(b",\"input\":");
        json_string(&mut req, src.as_bytes());
        req.push(b'}');

        let table_v = match self.call("parse", &req) {
            Err(e) => return Err(ErrorInfo::of(failure_text(e).as_bytes(), 0)),
            Ok(Answer::Refused(r)) => return Err(ErrorInfo::of(r.text().as_bytes(), r.byte_offset)),
            Ok(Answer::Result(r)) => r,
        };
        table_from_json(&table_v, out).map_err(|e| {
            ErrorInfo::of(if e == err!(MalformedTable) { b"the helper returned a malformed table" } else { e.name().as_bytes() }, 0)
        })
    }

    /// `print` with `{"dialect","table","options"}`: the text the helper
    /// answers. A refusal's code is `ErrorInfo::code` when it is a C
    /// `Refusal` (1 to 9), so a printer's reason reaches the caller as the
    /// failure it names.
    pub fn print_view(&self, dialect: &str, view: &table::View<'_>, options: &PrintOptions) -> std::result::Result<Vec<u8>, ErrorInfo> {
        if let Some(native) = &self.native_print {
            return native(dialect, view, options);
        }
        let mut req = Vec::new();
        req.extend_from_slice(b"{\"dialect\":");
        json_string(&mut req, dialect.as_bytes());
        req.extend_from_slice(b",\"table\":");
        table_to_json(&mut req, view);
        req.extend_from_slice(
            format!(
                ",\"options\":{{\"pretty\":{},\"strip_comments\":{},\"indent\":{},\"width\":{},\"splice\":{},\"flow\":{}}}}}",
                options.pretty, options.strip_comments, options.indent, options.width, options.splice, options.flow
            )
            .as_bytes(),
        );
        self.output_call("print", &req)
    }

    /// Call `method` with `params` and hand back the string it answers.
    fn output_call(&self, method: &str, params: &[u8]) -> std::result::Result<Vec<u8>, ErrorInfo> {
        let output = match self.call(method, params) {
            Err(e) => return Err(ErrorInfo::of(failure_text(e).as_bytes(), 0)),
            Ok(Answer::Refused(r)) => {
                let mut info = ErrorInfo::of(r.text().as_bytes(), 0);
                if r.code > 0 && r.code <= i32::MAX as i64 {
                    info.code = r.code as i32;
                }
                return Err(info);
            }
            Ok(Answer::Result(r)) => r,
        };
        match output {
            Value::String(s) => Ok(s.into_bytes()),
            _ => Err(ErrorInfo::of(b"the helper's output is not a string", 0)),
        }
    }

    /// `render` with `{"which","dialect","indent","key","value",…}`: the
    /// fragment the helper answers.
    pub fn render_call(&self, which: &str, dialect: &str, r: &RenderRequest<'_>) -> std::result::Result<Vec<u8>, ErrorInfo> {
        let mut req = Vec::new();
        req.extend_from_slice(format!("{{\"which\":\"{which}\",\"dialect\":").as_bytes());
        json_string(&mut req, dialect.as_bytes());
        for (name, v) in [("indent", r.indent), ("key", r.key), ("value", r.value)] {
            req.extend_from_slice(format!(",\"{name}\":").as_bytes());
            json_string(&mut req, v);
        }
        // What fig made of the value, for the value renderer alone.
        if which == "value" {
            req.extend_from_slice(b",\"literal\":");
            json_string(&mut req, r.literal.name().as_bytes());
        }
        for (name, v) in [("old_key", r.old_key), ("parent_key", r.parent_key), ("parent_tag", r.parent_tag)] {
            req.extend_from_slice(format!(",\"{name}\":").as_bytes());
            json_string(&mut req, v);
        }
        // The string replaced, for the string renderer alone.
        if which == "string" {
            req.extend_from_slice(b",\"old_value\":");
            json_string(&mut req, r.old_value);
        }
        req.push(b'}');
        self.output_call("render", &req)
    }

    /// `split_stream` with `{"dialect","input"}`, answered with
    /// `[{"start","end","explicit"}]` in byte offsets.
    pub fn split_stream_thunk(&self, dialect: &str, input: &[u8]) -> std::result::Result<Vec<StreamDocument>, ErrorInfo> {
        let Ok(src) = std::str::from_utf8(input) else {
            return Err(ErrorInfo::of(b"the input is not UTF-8, which the helper wire carries as text", 0));
        };
        let mut req = Vec::new();
        req.extend_from_slice(b"{\"dialect\":");
        json_string(&mut req, dialect.as_bytes());
        req.extend_from_slice(b",\"input\":");
        json_string(&mut req, src.as_bytes());
        req.push(b'}');

        let docs = match self.call("split_stream", &req) {
            Err(e) => return Err(ErrorInfo::of(failure_text(e).as_bytes(), 0)),
            Ok(Answer::Refused(r)) => return Err(ErrorInfo::of(r.text().as_bytes(), r.byte_offset)),
            Ok(Answer::Result(r)) => r,
        };
        let Value::Array(docs) = docs else {
            return Err(ErrorInfo::of(b"the helper's documents are not an array", 0));
        };
        let mut out = Vec::with_capacity(docs.len());
        for d in &docs {
            let (start, end) = match d {
                Value::Object(o) => (o.get("start"), o.get("end")),
                _ => (None, None),
            };
            match (start, end) {
                (Some(Value::Integer(s)), Some(Value::Integer(e))) if *s >= 0 && *e >= *s => {
                    let Value::Object(o) = d else { unreachable!() };
                    out.push(StreamDocument { start: *s as usize, end: *e as usize, explicit: bool_or(o, "explicit", false) });
                }
                _ => return Err(ErrorInfo::of(b"a stream document is not {\"start\", \"end\"} byte offsets", 0)),
            }
        }
        Ok(out)
    }
}

// ── the runtime `Language` over the thunks ─────────────────────────────────

impl Language for WireLanguage {
    fn name(&self) -> &str {
        self.name
    }

    fn extensions(&self) -> &[&'static str] {
        self.extensions
    }

    fn dialect_extensions(&self, row: usize) -> Option<Vec<&'static str>> {
        Some(self.dialect_extensions.get(row).cloned().unwrap_or_default())
    }

    fn caps(&self) -> Caps {
        self.caps
    }

    fn dialects(&self) -> &[DialectInfo] {
        self.dialects
    }

    fn samples(&self) -> &[&'static [u8]] {
        self.samples
    }

    /// The dialect's syntax, else the language's (the runtime registry's
    /// `Language.syntax`), else none at all for a read-only format.
    fn syntax(&self, dialect: Dialect) -> Syntax {
        if let Some(Some(s)) = self.dialect_syntax.get(dialect as usize) {
            return *s;
        }
        self.syntax.unwrap_or(Syntax::new(Comments::NONE, None, None))
    }

    fn is_runtime(&self) -> bool {
        true
    }

    /// Through the wire. A refusal is `RuntimeParseFailed`, its message and
    /// offset stated with `language::set_parse_error` (the `ErrorInfo` the
    /// registry's `Parser` reports).
    fn parse_table(&self, input: &[u8], dialect: Dialect, out: &mut Table) -> Result<()> {
        match self.parse_thunk(self.dialect_name(dialect), input, out) {
            Ok(()) => Ok(()),
            Err(info) => {
                language::set_parse_error(&info.message, info.byte_offset);
                Err(err!(RuntimeParseFailed))
            }
        }
    }

    /// `printNodeWith` over the wire: the tree's table (`document_to_table`;
    /// the registry has already dropped comments when asked), its C options,
    /// and the refusal's code as the failure it names.
    fn print(&self, dialect: Dialect, ast: &AST, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        if !self.caps.serialize {
            return Err(err!(FormatDisabled));
        }
        let mut built = convert::document_to_table(ast, ast.root)?;
        if options.strip_comments {
            built.comments.clear();
        }
        match self.print_view(self.dialect_name(dialect), &built.view(), &PrintOptions::of(options)) {
            Ok(bytes) => {
                out.extend_from_slice(&bytes);
                Ok(())
            }
            Err(info) => Err(if info.code >= 1 && info.code <= 9 { REFUSALS[info.code as usize - 1] } else { err!(RuntimePrintFailed) }),
        }
    }

    fn print_node(&self, dialect: Dialect, ast: &AST, node: NodeId, options: &SerializeOptions, out: &mut Vec<u8>) -> Result<()> {
        if !self.caps.serialize {
            return Err(err!(FormatDisabled));
        }
        let mut t = convert::document_to_table(ast, node)?;
        if options.strip_comments {
            t.comments.clear();
        }
        match self.print_view(self.dialect_name(dialect), &t.view(), &PrintOptions::of(options)) {
            Ok(bytes) => {
                out.extend_from_slice(&bytes);
                Ok(())
            }
            Err(info) => Err(if info.code >= 1 && info.code <= 9 { REFUSALS[info.code as usize - 1] } else { err!(RuntimePrintFailed) }),
        }
    }

    /// One renderer call, when the description lists it. A refusal is the
    /// helper's words in `registry::last_refusal` and `RendererRefused`
    /// (the runtime registry's `rendererError`).
    fn render(&self, which: Renderer, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        if !self.renderers[renderer_index(which)] {
            return None;
        }
        Some(match self.render_call(which.name(), self.dialect_name(dialect), r) {
            Ok(bytes) => {
                out.extend_from_slice(&bytes);
                Ok(())
            }
            Err(info) => {
                let text = String::from_utf8_lossy(info.text()).into_owned();
                registry::refuse(if text.is_empty() { "the renderer declined" } else { &text });
                Err(err!(RendererRefused))
            }
        })
    }

    fn render_value(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.render(Renderer::Value, dialect, out, r)
    }
    fn render_entry(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.render(Renderer::Entry, dialect, out, r)
    }
    fn render_item(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.render(Renderer::Item, dialect, out, r)
    }
    fn render_tail(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.render(Renderer::Tail, dialect, out, r)
    }
    fn render_key(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.render(Renderer::Key, dialect, out, r)
    }
    fn render_key_name(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.render(Renderer::KeyName, dialect, out, r)
    }
    fn render_string(&self, dialect: Dialect, out: &mut Vec<u8>, r: &RenderRequest<'_>) -> Option<Result<()>> {
        self.render(Renderer::String, dialect, out, r)
    }

    /// `Editor.hasRenderer`'s runtime answer: whether the description lists
    /// it (a null check of the vtable slot), without asking the helper.
    fn has_renderer(&self, _dialect: Dialect, which: Renderer) -> bool {
        self.renderers[renderer_index(which)]
    }

    /// `split_stream`, when the description says `split_stream: true`. A
    /// refusal is `RuntimeParseFailed` (the registry reads no message).
    fn split_stream(&self, input: &[u8], dialect: Dialect) -> Option<Result<Vec<StreamDocument>>> {
        if !self.split_stream {
            return None;
        }
        Some(self.split_stream_thunk(self.dialect_name(dialect), input).map_err(|_| err!(RuntimeParseFailed)))
    }

    fn has_split_stream(&self) -> bool {
        self.split_stream
    }
}

// ============================================================================
// A HELPER PROCESS
// ============================================================================

/// One spawned helper's pipes: the transport of the CLI's helper runner
/// (`src/cli/languages.zig`'s `Helper`). The helper's stderr is the CLI's.
pub struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process").field("pid", &self.child.id()).finish()
    }
}

/// An I/O error by the Zig name the same failure has.
fn io_error_name(e: &std::io::Error, otherwise: Error) -> Error {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => err!(FileNotFound),
        ErrorKind::PermissionDenied => err!(AccessDenied),
        ErrorKind::BrokenPipe => err!(BrokenPipe),
        _ => otherwise,
    }
}

impl Process {
    /// Start `argv` with its stdin and stdout piped and its stderr
    /// inherited. Fails with `HelperRefused`, the reason in
    /// `registry::last_refusal` (`could not start `…`: …`).
    pub fn spawn(argv: &[String]) -> Result<Process> {
        let Some(program) = argv.first() else {
            registry::refuse("could not start ``: FileNotFound");
            return Err(err!(HelperRefused));
        };
        let spawned = Command::new(program)
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                registry::refuse(&format!("could not start `{program}`: {}", io_error_name(&e, err!(SpawnFailed)).name()));
                return Err(err!(HelperRefused));
            }
        };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            registry::refuse(&format!("could not start `{program}`: SpawnFailed"));
            return Err(err!(HelperRefused));
        };
        Ok(Process { child, stdin, stdout: BufReader::with_capacity(64 * 1024, stdout) })
    }
}

impl Transport for Process {
    /// Send one request line and take one response line: a whole line,
    /// however long, its `\n` dropped; nothing at all is `HelperExited`.
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        let mut line = Vec::with_capacity(request.len() + 1);
        line.extend_from_slice(request);
        line.push(b'\n');
        self.stdin.write_all(&line).map_err(|e| io_error_name(&e, err!(WriteFailed)))?;
        self.stdin.flush().map_err(|e| io_error_name(&e, err!(WriteFailed)))?;
        let mut out = Vec::new();
        self.stdout.read_until(b'\n', &mut out).map_err(|e| io_error_name(&e, err!(ReadFailed)))?;
        if out.last() == Some(&b'\n') {
            out.pop();
        }
        if out.is_empty() {
            return Err(err!(HelperExited));
        }
        Ok(out)
    }
}

// ============================================================================
// THE TABLE ON THE WIRE
// ============================================================================

const KIND_NAMES: [&str; 9] = ["null", "bool", "int", "float", "string", "sequence", "mapping", "keyvalue", "alias"];

fn kind_of(name: &str) -> Option<usize> {
    KIND_NAMES.iter().position(|k| *k == name)
}

fn ext_kind_of(name: &str) -> Option<ExtKind> {
    ExtKind::ALL.into_iter().find(|k| k.name() == name)
}

/// The table kind of each wire kind but `bool`, which is two.
const WIRE_ROW_KINDS: [u8; 9] = [
    table::Kind::NULL,
    0,
    table::Kind::INT,
    table::Kind::FLOAT,
    table::Kind::STRING,
    table::Kind::SEQUENCE,
    table::Kind::MAPPING,
    table::Kind::KEYVALUE,
    table::Kind::ALIAS,
];

/// A `[start, end]` span, or `NONE` for an absent one.
fn span_of(v: Option<&Value>) -> Result<(u32, u32)> {
    let s = match v {
        None | Some(Value::Null) => return Ok((NONE, NONE)),
        Some(s) => s,
    };
    let Value::Array(a) = s else { return Err(err!(MalformedTable)) };
    if a.len() != 2 {
        return Err(err!(MalformedTable));
    }
    match (&a[0], &a[1]) {
        (Value::Integer(x), Value::Integer(y)) if *x >= 0 && *y >= 0 && *x < NONE as i64 && *y < NONE as i64 => {
            Ok((*x as u32, *y as u32))
        }
        _ => Err(err!(MalformedTable)),
    }
}

/// A string as the table's text, copied into the buffer.
fn text_of(t: &mut Table, v: Option<&Value>) -> Result<table::Text> {
    match v {
        None | Some(Value::Null) => Ok(table::Text::ABSENT),
        Some(Value::String(x)) => t.add_string(x.as_bytes()),
        Some(_) => Err(err!(MalformedTable)),
    }
}

fn u32_of(v: Option<&Value>) -> Result<u32> {
    match v {
        Some(Value::Integer(i)) if *i >= 0 && *i < NONE as i64 => Ok(*i as u32),
        _ => Err(err!(MalformedTable)),
    }
}

/// A mention's `kind` as the table's byte: `header`, `entry`, `index` or
/// `append`, and `header` for anything else, as the wire has always read a
/// kind it does not name.
fn mention_kind(v: &Value) -> u8 {
    let kind = match v {
        Value::String(s) => match s.as_str() {
            "entry" => MentionKind::Entry,
            "index" => MentionKind::Index,
            "append" => MentionKind::Append,
            _ => MentionKind::Header,
        },
        _ => MentionKind::Header,
    };
    kind.byte()
}

fn mention_name(b: u8) -> &'static str {
    match MentionKind::of(b) {
        MentionKind::Header => "header",
        MentionKind::Entry => "entry",
        MentionKind::Index => "index",
        MentionKind::Append => "append",
    }
}

/// The table a helper answers `parse` with, decoded into `out`.
/// `MalformedTable` for a shape the wire does not document. The wire spells
/// a row as it always has — a bool's text is `true`/`false`, an extended
/// scalar's kind is `ext_kind` beside a `string` (or `int`) row, and each
/// side fact rides on its row — so a helper written before the ABI 4 table
/// needs no change.
pub fn table_from_json(v: &Value, out: &mut Table) -> Result<()> {
    let Value::Object(o) = v else { return Err(err!(MalformedTable)) };
    if let Some(rv) = o.get("rows") {
        let Value::Array(rows) = rv else { return Err(err!(MalformedTable)) };
        for (i, r) in rows.iter().enumerate() {
            let Value::Object(ro) = r else { return Err(err!(MalformedTable)) };
            let node = i as u32;
            let Some(Value::String(kind_name)) = ro.get("kind") else { return Err(err!(MalformedTable)) };
            let wire_kind = kind_of(kind_name).ok_or(err!(MalformedTable))?;
            let parent = match ro.get("parent") {
                None | Some(Value::Null) => NONE,
                p => u32_of(p)?,
            };
            let (start, end) = span_of(ro.get("span"))?;
            let mut row = table::Row { parent, start, end, ..table::Row::default() };
            if let Some(e) = ro.get("ext_kind") {
                let Value::String(e) = e else { return Err(err!(MalformedTable)) };
                row.kind = table::Kind::EXT + ext_kind_of(e).ok_or(err!(MalformedTable))?.ordinal();
                row.text = text_of(out, ro.get("text"))?;
            } else if wire_kind == 1 {
                // bool
                match ro.get("text") {
                    Some(Value::String(b)) if b == "true" => row.kind = table::Kind::TRUE,
                    Some(Value::String(b)) if b == "false" => row.kind = table::Kind::FALSE,
                    _ => return Err(err!(MalformedTable)),
                }
            } else {
                row.kind = WIRE_ROW_KINDS[wire_kind];
                row.text = text_of(out, ro.get("text"))?;
            }
            if bool_or(ro, "open", false) {
                row.flags |= table::ROW_OPEN;
            }
            out.add_row(row)?;
            if let Some(an) = ro.get("anchor") {
                if *an != Value::Null {
                    let (start, end) = span_of(ro.get("anchor_span"))?;
                    let name = text_of(out, Some(an))?;
                    out.anchors.push(table::Anchor { node, start, end, name });
                }
            }
            if let Some(tg) = ro.get("tag") {
                if *tg != Value::Null {
                    let (start, end) = span_of(ro.get("tag_span"))?;
                    let text = text_of(out, Some(tg))?;
                    out.tags.push(table::Tag { node, start, end, text, ..table::Tag::default() });
                }
            }
            if let Some(m) = ro.get("marker") {
                if *m != Value::Null {
                    let (start, end) = span_of(Some(m))?;
                    out.markers.push(table::SpanFact { node, start, end });
                }
            }
            if let Some(m) = ro.get("sep") {
                if *m != Value::Null {
                    let (start, end) = span_of(Some(m))?;
                    out.seps.push(table::SpanFact { node, start, end });
                }
            }
            if let Some(is) = ro.get("item_sep") {
                if *is != Value::Null {
                    let text = text_of(out, Some(is))?;
                    out.item_seps.push(table::ItemSep { node, text });
                }
            }
        }
    }
    if let Some(Value::Array(rv)) = o.get("regions") {
        for r in rv {
            let Value::Object(r) = r else { return Err(err!(MalformedTable)) };
            let node = u32_of(r.get("node"))?;
            let start = u32_of(r.get("start"))?;
            let end = u32_of(r.get("end"))?;
            out.regions.push(table::SpanFact { node, start, end });
        }
    }
    if let Some(Value::Array(mv)) = o.get("mentions") {
        for m in mv {
            let Value::Object(m) = m else { return Err(err!(MalformedTable)) };
            let k = m.get("kind").ok_or(err!(MalformedTable))?;
            let (start, end) = span_of(m.get("span"))?;
            let node = u32_of(m.get("node"))?;
            out.mentions.push(table::Mention { node, start, end, kind: mention_kind(k) });
        }
    }
    if let Some(Value::Array(cv)) = o.get("comments") {
        for c in cv {
            let Value::Object(c) = c else { return Err(err!(MalformedTable)) };
            let (Some(Value::String(slot)), Some(Value::String(style))) = (c.get("slot"), c.get("style")) else {
                return Err(err!(MalformedTable));
            };
            let slot = match slot.as_str() {
                "trailing" => table::COMMENT_TRAILING,
                "dangling" => table::COMMENT_DANGLING,
                _ => table::COMMENT_LEADING,
            };
            let style = if style == "block" { table::COMMENT_BLOCK } else { table::COMMENT_LINE };
            let mut text = text_of(out, c.get("text"))?;
            if text.is_absent() {
                text = out.add_string(b"")?;
            }
            let node = u32_of(c.get("node"))?;
            out.comments.push(table::Comment { node, slot, style, text });
        }
    }
    if let Some(Value::Array(dv)) = o.get("diagnostics") {
        for d in dv {
            let Value::Object(d) = d else { return Err(err!(MalformedTable)) };
            let (start, end) = span_of(d.get("span"))?;
            let warning = matches!(d.get("severity"), Some(Value::String(s)) if s == "warning");
            let message = text_of(out, d.get("message"))?;
            let label = text_of(out, d.get("label"))?;
            out.diagnostics.push(table::Diagnostic {
                severity: if warning { table::DIAGNOSTIC_WARNING } else { table::DIAGNOSTIC_ERROR },
                start: if start == NONE { 0 } else { start },
                end,
                message,
                label,
            });
        }
    }
    if let Some(Value::Array(dv)) = o.get("directives") {
        for d in dv {
            let Value::Object(d) = d else { return Err(err!(MalformedTable)) };
            let handle = text_of(out, d.get("handle"))?;
            let prefix = text_of(out, d.get("prefix"))?;
            if handle.is_absent() || prefix.is_absent() {
                return Err(err!(MalformedTable));
            }
            out.directives.push(table::Directive { handle, prefix });
        }
    }
    Ok(())
}

fn write_span(w: &mut Vec<u8>, key: &str, start: u32, end: u32) {
    if start != NONE {
        w.extend_from_slice(format!(",\"{key}\":[{start},{end}]").as_bytes());
    }
}

fn write_text(w: &mut Vec<u8>, v: &table::View<'_>, key: &str, t: table::Text) {
    if let Some(x) = v.text_of(t) {
        w.extend_from_slice(format!(",\"{key}\":").as_bytes());
        json_string(w, x);
    }
}

/// A table as the JSON a helper answers `parse` with and is handed for
/// `print`: an absent optional is omitted, not `null`. The rows' side facts
/// ride on their rows, as the wire has always spelled them.
pub fn table_to_json(w: &mut Vec<u8>, v: &table::View<'_>) {
    use table::Kind as K;
    w.extend_from_slice(b"{\"rows\":[");
    for (i, r) in v.row_slice().iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        let node = i as u32;
        if r.kind >= K::EXT {
            let ek = ExtKind::from_ordinal(r.kind - K::EXT);
            let (kind, name) = match ek {
                Some(ExtKind::CharLiteral) => ("int", "char_literal"),
                Some(k) => ("string", k.name()),
                None => ("string", ""),
            };
            w.extend_from_slice(format!("{{\"kind\":\"{kind}\",\"ext_kind\":\"{name}\"").as_bytes());
        } else {
            let name = match r.kind {
                K::NULL => "null",
                K::FALSE | K::TRUE => "bool",
                K::STRING => "string",
                K::INT => "int",
                K::FLOAT => "float",
                K::SEQUENCE => "sequence",
                K::MAPPING => "mapping",
                K::KEYVALUE => "keyvalue",
                K::ALIAS => "alias",
                _ => "null",
            };
            w.extend_from_slice(format!("{{\"kind\":\"{name}\"").as_bytes());
        }
        if r.parent == NONE {
            w.extend_from_slice(b",\"parent\":null");
        } else {
            w.extend_from_slice(format!(",\"parent\":{}", r.parent).as_bytes());
        }
        write_span(w, "span", r.start, r.end);
        if r.kind == K::FALSE {
            w.extend_from_slice(b",\"text\":\"false\"");
        } else if r.kind == K::TRUE {
            w.extend_from_slice(b",\"text\":\"true\"");
        } else {
            write_text(w, v, "text", r.text);
        }
        let f = v.facts(node);
        if let Some(a) = f.anchor {
            write_text(w, v, "anchor", a.name);
            write_span(w, "anchor_span", a.start, a.end);
        }
        if let Some(t) = f.tag {
            write_text(w, v, "tag", t.text);
            write_span(w, "tag_span", t.start, t.end);
        }
        if let Some(m) = f.marker {
            write_span(w, "marker", m.start, m.end);
        }
        if let Some(m) = f.sep {
            write_span(w, "sep", m.start, m.end);
        }
        if let Some(is) = f.item_sep {
            write_text(w, v, "item_sep", is.text);
        }
        if r.flags & table::ROW_OPEN != 0 {
            w.extend_from_slice(b",\"open\":true");
        }
        w.push(b'}');
    }
    w.extend_from_slice(b"],\"regions\":[");
    for (i, r) in v.regions.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        w.extend_from_slice(format!("{{\"node\":{},\"start\":{},\"end\":{}}}", r.node, r.start, r.end).as_bytes());
    }
    w.extend_from_slice(b"],\"mentions\":[");
    for (i, m) in v.mentions.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        w.extend_from_slice(
            format!("{{\"node\":{},\"span\":[{},{}],\"kind\":\"{}\"}}", m.node, m.start, m.end, mention_name(m.kind)).as_bytes(),
        );
    }
    w.extend_from_slice(b"],\"comments\":[");
    for (i, c) in v.comments.iter().enumerate() {
        if i > 0 {
            w.push(b',');
        }
        let slot = match c.slot {
            table::COMMENT_TRAILING => "trailing",
            table::COMMENT_DANGLING => "dangling",
            _ => "leading",
        };
        let style = if c.style == table::COMMENT_BLOCK { "block" } else { "line" };
        w.extend_from_slice(format!("{{\"node\":{},\"slot\":\"{slot}\",\"style\":\"{style}\",\"text\":", c.node).as_bytes());
        json_string(w, v.text_of(c.text).unwrap_or(b""));
        w.push(b'}');
    }
    w.push(b']');
    // Absent, not empty, for the formats that have none — which is every
    // helper written before the column existed.
    if !v.directives.is_empty() {
        w.extend_from_slice(b",\"directives\":[");
        for (i, d) in v.directives.iter().enumerate() {
            if i > 0 {
                w.push(b',');
            }
            w.extend_from_slice(b"{\"handle\":");
            json_string(w, v.text_of(d.handle).unwrap_or(b""));
            w.extend_from_slice(b",\"prefix\":");
            json_string(w, v.text_of(d.prefix).unwrap_or(b""));
            w.push(b'}');
        }
        w.push(b']');
    }
    w.push(b'}');
}

// The literal names a value renderer is told (`manifest::Literal::name`) are
// the wire's own; nothing to translate.
const _: fn(Literal) -> &'static str = Literal::name;

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc as StdArc;

    /// A transport that answers from a fixed script — what a helper would
    /// say — so the language's half of the wire is tested with no process
    /// behind it. Each script is the member a response carries after its
    /// id: a `"result":…` or an `"error":…`, which `exchange` wraps in the
    /// envelope with the request's own id.
    #[derive(Clone)]
    pub(crate) struct Scripted {
        pub describe_response: &'static str,
        pub parse_response: &'static str,
        pub print_response: &'static str,
        pub last_request: StdArc<Mutex<Vec<u8>>>,
        id: i64,
    }

    impl Scripted {
        pub(crate) fn new(describe: &'static str, parse: &'static str, print: &'static str) -> Scripted {
            Scripted {
                describe_response: describe,
                parse_response: parse,
                print_response: print,
                last_request: StdArc::new(Mutex::new(Vec::new())),
                id: 0,
            }
        }

        fn last(&self) -> Vec<u8> {
            self.last_request.lock().unwrap().clone()
        }

        /// The request's `params`, as the request spells them.
        fn params(&self) -> String {
            let req = String::from_utf8(self.last()).unwrap();
            let Some(at) = req.find("\"params\": ") else { return String::new() };
            let Some(id_at) = req.rfind(", \"id\": ") else { return String::new() };
            req[at + "\"params\": ".len()..id_at].to_string()
        }
    }

    impl Transport for Scripted {
        fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>> {
            *self.last_request.lock().unwrap() = request.to_vec();
            self.id += 1;
            let req = String::from_utf8_lossy(request);
            let member = if req.contains("\"method\": \"describe\"") {
                self.describe_response
            } else if req.contains("\"method\": \"parse\"") {
                self.parse_response
            } else {
                self.print_response
            };
            Ok(format!("{{\"jsonrpc\":\"2.0\",\"id\":{},{member}}}", self.id).into_bytes())
        }
    }

    /// A described language with the given scripts behind it.
    fn language(s: &Scripted, describe_response: &'static str) -> WireLanguage {
        let mut t = s.clone();
        t.describe_response = describe_response;
        describe(Box::new(t)).unwrap()
    }

    const WIRE_KV: &str = "\"result\":{\"name\":\"wire-kv\",\"caps\":{\"read\":true,\"edit\":true,\"serialize\":true},\
\"max_mapping_depth\":0,\"syntax\":{\"comments\":{\"line\":{\"open\":\"#\"},\"trailing\":{\"open\":\"#\"}},\
\"kv_sep\":\"=\",\"empty_map_literal\":\"{}\",\"flow_containers\":false},\
\"dialects\":[{\"name\":\"wire-kv\",\"extensions\":[\"wkv\"],\"splice\":\"raw\",\"empty_doc_seed\":\"\"}],\
\"samples\":[\"a=1\\n\"],\"renderers\":[\"value\"]}";

    #[test]
    fn read_answer_a_result_a_refusal_with_its_offset_and_every_way_a_line_is_not_an_answer() {
        let ok = read_answer(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"n\":-3,\"f\":1.5,\"s\":\"a\\nb\"}}", 3).unwrap();
        let Answer::Result(r) = ok else { panic!() };
        assert_eq!(Some(-3), r.get("n").unwrap().as_i64());
        assert_eq!(&Value::NumberString("1.5".into()), r.get("f").unwrap());
        assert_eq!(Some("a\nb"), r.get("s").unwrap().as_str());

        let Answer::Refused(no) =
            read_answer(b"{\"jsonrpc\":\"2.0\",\"id\":4,\"error\":{\"code\":-32000,\"message\":\"bad\",\"data\":{\"byte_offset\":7}}}", 4)
                .unwrap()
        else {
            panic!()
        };
        assert_eq!(-32000, no.code);
        assert_eq!("bad", no.text());
        assert_eq!(7, no.byte_offset);

        let Answer::Refused(unknown) =
            read_answer(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"error\":{\"code\":-32601,\"message\":\"no such method\"}}", 5).unwrap()
        else {
            panic!()
        };
        assert!(unknown.text().contains("could not handle"));

        assert_eq!(err!(HelperSpeaksOldWire), read_answer(b"{\"ok\":true,\"output\":\"x\"}", 1).unwrap_err());
        assert_eq!(err!(HelperOutOfStep), read_answer(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":1}", 1).unwrap_err());
        assert_eq!(err!(HelperSpokeNoJson), read_answer(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1} trailing", 1).unwrap_err());
        assert_eq!(err!(HelperSpokeNoJson), read_answer(b"not json", 1).unwrap_err());
        assert_eq!(err!(HelperSpokeNoJson), read_answer(b"[]", 1).unwrap_err());
    }

    #[test]
    fn describe_builds_a_language_the_registry_would_accept() {
        let s = Scripted::new("", "", "");
        let l = language(&s, WIRE_KV);
        assert_eq!("wire-kv", l.name());
        let caps = l.caps();
        assert!(caps.read && caps.edit && caps.serialize && !caps.references);
        assert_eq!(Some(0), caps.max_mapping_depth);
        assert_eq!(1, l.dialects().len());
        assert_eq!(Some(vec!["wkv"]), l.dialect_extensions(0));
        assert_eq!(SpliceStyle::Raw, l.dialects()[0].splice);
        assert_eq!(Some(&b"="[..]), l.syntax(0).kv_sep);
        assert!(!l.syntax(0).flow_containers);
        assert!(l.has_renderer(0, Renderer::Value));
        assert!(!l.has_renderer(0, Renderer::Entry));
        assert!(!l.has_split_stream());
        // The description's language passes the registry's validation.
        assert_eq!(Ok(()), registry::validate_language(&l));
        assert_eq!(Ok(()), l.validate_record());
    }

    #[test]
    fn a_render_request_carries_the_container_the_fragment_goes_into() {
        let s = Scripted::new("", "", "\"result\":\"<dependency>x</dependency>\"");
        let l = language(&s, WIRE_KV);
        let request = RenderRequest {
            indent: b"  ",
            value: b"x",
            literal: Literal::String,
            parent_key: b"dependencies",
            parent_tag: b"!dependency",
            ..RenderRequest::default()
        };
        let out = l.render_call("item", "wire-kv", &request).unwrap();
        assert_eq!(b"<dependency>x</dependency>", &out[..]);
        assert!(String::from_utf8(s.last()).unwrap().contains("\"method\": \"render\""));
        assert_eq!(
            "{\"which\":\"item\",\"dialect\":\"wire-kv\",\"indent\":\"  \",\"key\":\"\",\"value\":\"x\",\"old_key\":\"\",\"parent_key\":\"dependencies\",\"parent_tag\":\"!dependency\"}",
            s.params()
        );
    }

    #[test]
    fn a_refusal_to_describe_is_reported_through_refuse() {
        let s = Scripted::new("\"error\":{\"code\":-32000,\"message\":\"no script given\"}", "", "");
        assert_eq!(err!(HelperRefused), describe(Box::new(s)).unwrap_err());
        assert!(String::from_utf8_lossy(&registry::last_refusal()).contains("no script given"));
    }

    #[test]
    fn parse_crosses_as_a_request_line_and_comes_back_as_a_table_print_gets_the_table_back() {
        let s = Scripted::new(
            "",
            "\"result\":{\"rows\":[{\"kind\":\"mapping\",\"parent\":null,\"span\":[0,4]},\
{\"kind\":\"keyvalue\",\"parent\":0,\"span\":[0,3],\"sep\":[1,2]},{\"kind\":\"string\",\"parent\":1,\"span\":[0,1],\"text\":\"a\"},\
{\"kind\":\"string\",\"parent\":1,\"span\":[2,3],\"text\":\"1\"}],\"regions\":[],\"mentions\":[],\
\"comments\":[{\"node\":2,\"slot\":\"leading\",\"style\":\"line\",\"text\":\"hi\"}],\
\"directives\":[{\"handle\":\"!e!\",\"prefix\":\"tag:x/\"}]}",
            "\"result\":\"a=1\\n\"",
        );
        let l = language(&s, WIRE_KV);
        let mut table = Table::init(&b"a=1\n"[..]);
        l.parse_thunk("wire-kv", b"a=1\n", &mut table).unwrap();
        // The first request was `describe`; this is the second.
        assert_eq!(
            "{\"jsonrpc\": \"2.0\", \"method\": \"parse\", \"params\": {\"dialect\":\"wire-kv\",\"input\":\"a=1\\n\"}, \"id\": 2}",
            String::from_utf8(s.last()).unwrap()
        );
        assert_eq!(4, table.rows.len());
        assert_eq!(1, table.comments.len());
        assert_eq!(1, table.directives.len());
        assert_eq!(b"tag:x/", table.text_of(table.directives[0].prefix).unwrap());
        assert_eq!(b"a", table.text_of(table.rows[2].text).unwrap());
        assert_eq!(1, table.seps.len());
        assert_eq!(1, table.seps[0].node);
        assert_eq!(1, table.seps[0].start);

        let options = PrintOptions { pretty: true, strip_comments: false, indent: 2, width: 80, splice: false, flow: false };
        let out = l.print_view("wire-kv", &table.view(), &options).unwrap();
        assert_eq!(b"a=1\n", &out[..]);
        // The print request carries the table as `parse` answered it.
        let req = String::from_utf8(s.last()).unwrap();
        assert!(req.contains("\"sep\":[1,2]"));
        assert!(req.contains("\"slot\":\"leading\""));
        assert!(req.contains("\"directives\":[{\"handle\":\"!e!\",\"prefix\":\"tag:x/\"}]"));
        assert!(req.contains("\"options\":{\"pretty\":true,\"strip_comments\":false,\"indent\":2,\"width\":80,\"splice\":false,\"flow\":false}"));
    }

    #[test]
    fn a_parse_refusal_carries_the_helpers_message_and_offset() {
        let s = Scripted::new("", "\"error\":{\"code\":-32000,\"message\":\"expected `=` here\",\"data\":{\"byte_offset\":3}}", "");
        let l = language(&s, WIRE_KV);
        let mut table = Table::init(&b"abc"[..]);
        let info = l.parse_thunk("wire-kv", b"abc", &mut table).unwrap_err();
        assert_eq!(b"expected `=` here", info.text());
        assert_eq!(3, info.byte_offset);

        // Through the trait and the registry's `Parser`, the same words.
        let r = Registry::new();
        let lang = language(&s, WIRE_KV);
        let abi = r.register_arc(StdArc::new(lang), RegisterOptions { harness: false }).unwrap();
        let e = r.entry_by_abi(abi).unwrap();
        let mut p = registry::Parser::new();
        assert_eq!(err!(RuntimeParseFailed), p.parse(b"abc", &e).unwrap_err());
        assert_eq!(b"expected `=` here", p.last_message());
        assert_eq!(3, p.last_offset);
        assert!(p.errors.is_empty());
    }

    // ── beyond the Zig tests: the runtime half ─────────────────────────────

    #[test]
    fn a_print_refusal_code_is_the_failure_it_names() {
        let s = Scripted::new("", "", "\"error\":{\"code\":2,\"message\":\"keys are strings\"}");
        let l = language(&s, WIRE_KV);
        let doc = crate::lang::json::LANGUAGE.parse(b"{\"a\":1}", 0).unwrap();
        assert_eq!(err!(NonStringKey), l.print(0, &doc.ast, &SerializeOptions::default(), &mut Vec::new()).unwrap_err());
        let s = Scripted::new("", "", "\"error\":{\"code\":-32000,\"message\":\"no\"}");
        let l = language(&s, WIRE_KV);
        assert_eq!(err!(RuntimePrintFailed), l.print(0, &doc.ast, &SerializeOptions::default(), &mut Vec::new()).unwrap_err());
    }

    #[test]
    fn a_renderer_refusal_keeps_the_helpers_words() {
        let s = Scripted::new("", "", "\"error\":{\"code\":-32000,\"message\":\"no spelling for that\"}");
        let r = Registry::new();
        let abi = r.register_arc(StdArc::new(language(&s, WIRE_KV)), RegisterOptions { harness: false }).unwrap();
        let e = r.entry_by_abi(abi).unwrap();
        assert_eq!(err!(RendererRefused), e.render(Renderer::Value, &mut Vec::new(), &RenderRequest::default()).unwrap_err());
        assert_eq!(b"no spelling for that", &registry::last_refusal()[..]);
        assert_eq!(err!(UnsupportedShape), e.render(Renderer::Entry, &mut Vec::new(), &RenderRequest::default()).unwrap_err());
    }

    #[test]
    fn an_editable_description_without_syntax_or_with_half_a_record_is_refused() {
        let s = Scripted::new("", "", "");
        let l = language(
            &s,
            "\"result\":{\"name\":\"nosyn\",\"caps\":{\"read\":true,\"edit\":true},\"dialects\":[{\"name\":\"nosyn\"}],\"samples\":[\"x\"]}",
        );
        assert_eq!(err!(InvalidLanguage), register(&Registry::new(), l, RegisterOptions::default()).unwrap_err());
        assert!(String::from_utf8_lossy(&registry::last_refusal()).contains("must supply syntax"));
        let l = language(
            &s,
            "\"result\":{\"name\":\"half\",\"caps\":{\"read\":true,\"edit\":true},\"syntax\":{\"kv_sep\":\"=\",\
\"closed_containers\":{\"map_open\":\"<\"}},\"dialects\":[{\"name\":\"half\"}],\"samples\":[\"x\"]}",
        );
        assert_eq!(err!(InvalidLanguage), register(&Registry::new(), l, RegisterOptions::default()).unwrap_err());
        assert!(String::from_utf8_lossy(&registry::last_refusal()).contains("closed_containers needs all four tokens"));
    }

    #[test]
    fn a_description_that_is_not_one_is_spoken_no_json() {
        let s = Scripted::new("", "", "");
        let mut t = s.clone();
        t.describe_response = "\"result\":{\"caps\":{}}";
        assert_eq!(err!(HelperSpokeNoJson), describe(Box::new(t)).unwrap_err());
        let mut t = s.clone();
        t.describe_response = "\"result\":[1]";
        assert_eq!(err!(HelperSpokeNoJson), describe(Box::new(t)).unwrap_err());
    }

    #[test]
    fn json_reads_strictly_and_keeps_non_integers_as_text() {
        let v = json::parse(b" {\"a\":[1,-2,3.5,1e3,99999999999999999999],\"b\":\"x\\u00e9\\ud83d\\ude00\",\"a\":0} ").unwrap();
        assert_eq!(Some(0), v.get("a").unwrap().as_i64());
        assert_eq!(2, v.as_object().unwrap().len());
        assert_eq!(Some("x\u{e9}\u{1F600}"), v.get("b").unwrap().as_str());
        let v = json::parse(b"[1,-2,3.5,1e3,99999999999999999999]").unwrap();
        let items = v.as_array().unwrap();
        assert_eq!(Some(-2), items[1].as_i64());
        assert_eq!(Value::NumberString("1e3".into()), items[3]);
        assert_eq!(Value::NumberString("99999999999999999999".into()), items[4]);
        for bad in [&b"01"[..], b"[1,]", b"{\"a\" 1}", b"\"\x01\"", b"\"\\ud800\"", b"tru", b"1 2", b"-", b"1.", b"\"\xff\""] {
            assert!(json::parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        // Depth within a message's value is bounded.
        let deep = format!("{}{}", "[".repeat(json::MAX_DEPTH + 2), "]".repeat(json::MAX_DEPTH + 2));
        let line = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{deep}}}");
        assert_eq!(err!(HelperSpokeNoJson), read_answer(line.as_bytes(), 1).unwrap_err());
    }
}
