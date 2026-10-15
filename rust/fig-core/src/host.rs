//! Port of src/contract/host.zig: a runtime language whose functions live in
//! the wasm module's host. The language is `crate::wire`'s — the helper
//! wire, JSON-RPC 2.0 by the line — over a transport whose one call is a
//! wasm import, `fig_host.call`, answered by the same `handle` a helper
//! process runs over stdin and stdout.
//!
//! Two modules register this way. The reactor the TypeScript binding loads
//! registers a language when the binding asks (`wasm_host.zig`'s
//! `fig_host_language_register`); the CLI built for WASI registers one when
//! it meets a format it did not compile in and its host has a language by
//! that name (`cli/host_languages.zig`). Each declares the import itself —
//! an `extern` is a symbol of the module that references it — and hands it
//! here, wrapped in a safe function of the shape `Call` (the wrapper reads
//! the host's bytes and frees them, which in Zig is this module's
//! `std.heap.wasm_allocator`).
//!
//! # Public API
//!
//! ```text
//! pub type Call = fn(lang: u32, request: &[u8]) -> Option<Vec<u8>>;
//! pub type Parse = fn(lang: u32, dialect: &str, input: &[u8], out: &mut Table) -> Result<(), ErrorInfo>;
//! pub type Print = fn(lang: u32, dialect: &str, view: &View<'_>, options: &PrintOptions) -> Result<Vec<u8>, ErrorInfo>;
//! pub fn register(call: Call, lang: u32) -> Result<i32>;                       // into registry::global()
//! pub fn register_native(call: Call, parse: Parse, print: Print, lang: u32) -> Result<i32>;
//! pub fn register_in(registry: &Registry, call: Call, parse: Option<Parse>, print: Option<Print>, lang: u32) -> Result<i32>;
//! // Errors (RegisterError): Refused (`registry::last_refusal` says why),
//! // Malformed, RegistryFull.
//! ```

use crate::err;
use crate::kit::table::{self, Table};
use crate::registry::{self, RegisterOptions, Registry};
use crate::wire::{self, ErrorInfo, PrintOptions, Transport};
use crate::Result;

/// The shape of `fig_host.call`: one request line in, one response line
/// out. `lang` is the host's own id for the language. `None` is a host that
/// could not answer at all — a refusal is an ordinary JSON-RPC error
/// response.
pub type Call = fn(lang: u32, request: &[u8]) -> Option<Vec<u8>>;

/// A language in a WebAssembly module of its own (`kit/wasm_language.zig`):
/// the host parses `input` in `dialect` there and writes the table into
/// `out` (`wasm_host.zig`'s `fig_table_extend`), or says why not.
pub type Parse = fn(lang: u32, dialect: &str, input: &[u8], out: &mut Table) -> std::result::Result<(), ErrorInfo>;

/// The host prints the table `view` shows in `dialect` there, handing the
/// text back.
pub type Print =
    fn(lang: u32, dialect: &str, view: &table::View<'_>, options: &PrintOptions) -> std::result::Result<Vec<u8>, ErrorInfo>;

/// The transport for one registered language. Lives for the process, as a
/// registration does.
struct HostLanguage {
    call: Call,
    lang: u32,
}

impl Transport for HostLanguage {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        (self.call)(self.lang, request).ok_or(err!(HelperExited))
    }
}

/// `RegisterError`: `OutOfMemory`; `Refused` (the host's description was
/// refused, or the harness failed over its samples, or its name is taken:
/// `registry::last_refusal()` says which); `Malformed` (the description is
/// not the shape the wire documents); `RegistryFull`.
pub const REGISTER_ERRORS: &[&str] = &["OutOfMemory", "Refused", "Malformed", "RegistryFull"];

/// Register the language the host knows as `lang`: ask it to `describe`
/// itself through `call`, build the language, and register that through
/// the same `wire::register` a helper process goes through — which
/// validates the description and runs the harness over its samples, calling
/// back into the host for each parse. Returns the format integer of the
/// first dialect row.
pub fn register(call: Call, lang: u32) -> Result<i32> {
    register_in(registry::global(), call, None, None, lang)
}

/// `register` for a language in a WebAssembly module of its own: described
/// and rendered by the wire, as any host language is, but parsed and
/// printed through `parse` and `print`, which move the table between the
/// two modules' memories as the rows it is, not as JSON.
pub fn register_native(call: Call, parse: Parse, print: Print, lang: u32) -> Result<i32> {
    register_in(registry::global(), call, Some(parse), Some(print), lang)
}

/// `register`/`register_native` into `registry` (Zig's `registerWith`, over
/// the one registry Zig has).
pub fn register_in(registry: &Registry, call: Call, parse: Option<Parse>, print: Option<Print>, lang: u32) -> Result<i32> {
    let host = HostLanguage { call, lang };
    let mut described = match wire::describe(Box::new(host)) {
        Ok(l) => l,
        Err(e) if e == err!(HelperRefused) => return Err(err!(Refused)),
        Err(e) if e == err!(HelperSpokeNoJson) => return Err(err!(Malformed)),
        Err(e) => return Err(e),
    };
    if let Some(parse) = parse {
        described.native_parse = Some(Box::new(move |dialect, input, out| parse(lang, dialect, input, out)));
    }
    if let Some(print) = print {
        if crate::kit::language::Language::caps(&described).serialize {
            described.native_print = Some(Box::new(move |dialect, view, options| print(lang, dialect, view, options)));
        }
    }
    match wire::register(registry, described, RegisterOptions::default()) {
        Ok(abi) => Ok(abi),
        Err(e) if e == err!(InvalidLanguage) || e == err!(HarnessFailed) || e == err!(NameTaken) => Err(err!(Refused)),
        Err(e) if e == err!(RegistryFull) => Err(err!(RegistryFull)),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::PathSegment;
    use crate::kit::language::Language;

    /// A host with one language, `host-kv`, answering from a script: what
    /// `fig_host.call` reaches in JavaScript. Its parse is the Rust kit's
    /// `key=value` lines.
    fn host_call(_lang: u32, request: &[u8]) -> Option<Vec<u8>> {
        let req = wire::json::parse(request).ok()?;
        let id = req.get("id")?.as_i64()?;
        let member = match req.get("method")?.as_str()? {
            "describe" => "\"result\":{\"name\":\"host-kv\",\"caps\":{\"read\":true,\"edit\":true,\"serialize\":true},\
\"max_mapping_depth\":0,\"syntax\":{\"kv_sep\":\"=\",\"flow_containers\":false},\
\"dialects\":[{\"name\":\"host-kv\",\"extensions\":[\"hkv\"],\"splice\":\"raw\",\"empty_doc_seed\":\"\"}],\
\"samples\":[\"a=1\\n\"]}"
                .to_string(),
            "parse" => {
                let input = req.get("params")?.get("input")?.as_str()?;
                let Some(eq) = input.find('=') else {
                    return Some(
                        format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32000,\"message\":\"no =\",\"data\":{{\"byte_offset\":0}}}}}}")
                            .into_bytes(),
                    );
                };
                let end = input.trim_end_matches('\n').len();
                format!(
                    "\"result\":{{\"rows\":[{{\"kind\":\"mapping\",\"parent\":null,\"span\":[0,{}]}},\
{{\"kind\":\"keyvalue\",\"parent\":0,\"span\":[0,{end}],\"sep\":[{eq},{}]}},\
{{\"kind\":\"string\",\"parent\":1,\"span\":[0,{eq}],\"text\":\"{}\"}},\
{{\"kind\":\"string\",\"parent\":1,\"span\":[{},{end}],\"text\":\"{}\"}}]}}",
                    input.len(),
                    eq + 1,
                    &input[..eq],
                    eq + 1,
                    &input[eq + 1..end]
                )
            }
            "print" => {
                let t = req.get("params")?.get("table")?;
                let rows = t.get("rows")?.as_array()?;
                let k = rows.get(2)?.get("text")?.as_str()?;
                let v = rows.get(3)?.get("text")?.as_str()?;
                format!("\"result\":\"{k}={v}\\n\"")
            }
            _ => return None,
        };
        Some(format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},{member}}}").into_bytes())
    }

    fn dead_call(_lang: u32, _request: &[u8]) -> Option<Vec<u8>> {
        None
    }

    #[test]
    fn a_host_language_registers_parses_and_prints_through_its_call() {
        let r = Registry::new();
        let abi = register_in(&r, host_call, None, None, 7).unwrap();
        let e = r.entry_by_abi(abi).unwrap();
        assert_eq!("host-kv", e.name);
        assert_eq!(vec!["hkv"], e.extensions);
        let doc = e.parse(b"k=v\n").unwrap();
        assert_eq!(b"v", doc.ast.get_val_by_path(&[PathSegment::Key(b"k")]).unwrap().kind.as_string().unwrap());
        let mut out = Vec::new();
        registry::print_with(&e, &doc.ast, &Default::default(), &mut out).unwrap();
        assert_eq!(b"k=v\n", &out[..]);
        let mut p = registry::Parser::new();
        assert!(p.parse(b"nope\n", &e).is_err());
        assert_eq!(b"no =", p.last_message());
        // A second registration of the same name is refused.
        assert_eq!(err!(Refused), register_in(&r, host_call, None, None, 7).unwrap_err());
    }

    #[test]
    fn a_host_that_cannot_answer_is_refused() {
        assert_eq!(err!(Refused), register_in(&Registry::new(), dead_call, None, None, 1).unwrap_err());
        assert!(String::from_utf8_lossy(&registry::last_refusal()).contains("HelperExited"));
    }

    fn native_parse(_lang: u32, _dialect: &str, input: &[u8], out: &mut Table) -> std::result::Result<(), ErrorInfo> {
        crate::lang::dotenv::LANGUAGE.parse_table(input, 0, out).map_err(|e| ErrorInfo { message: e.name().into(), ..Default::default() })
    }

    fn native_print(_lang: u32, _dialect: &str, _view: &table::View<'_>, _options: &PrintOptions) -> std::result::Result<Vec<u8>, ErrorInfo> {
        Ok(b"a=1\n".to_vec())
    }

    #[test]
    fn a_native_host_language_parses_and_prints_apart_from_the_wire() {
        let r = Registry::new();
        let abi = register_in(&r, host_call, Some(native_parse), Some(native_print), 3).unwrap();
        let e = r.entry_by_abi(abi).unwrap();
        // The native parse is dotenv's, which the wire's parse is not.
        let doc = e.parse(b"A=\"q\"\n").unwrap();
        assert_eq!(b"q", doc.ast.get_val_by_path(&[PathSegment::Key(b"A")]).unwrap().kind.as_string().unwrap());
        assert!(e.lang().caps().serialize);
    }
}
