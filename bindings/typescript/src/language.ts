// Runtime languages: a format written in JavaScript, registered while the
// program runs, and from then on a `Format` every fig API accepts.
//
// The object and the wire are `wire.ts`; this is the registration — what
// needs the wasm module. The module reaches the object through one import,
// `fig_host.call`: a request line in, a response line out — `handle`, the
// same function `serve` runs over stdin and stdout. The module builds a
// vtable over it and registers that through the same path a C host's
// vtable takes; the module's own formats and a JavaScript one are peers
// from then on. See docs/proposals/runtime-languages.md §7.3 in the core.
import { Frame, allocFigError, fig, readFigError, readU32, setHostCall, u8, dv, encoder, decoder } from "./ffi.ts";
import { FigError, Status, type Format } from "./types.ts";
import { handle, type Language } from "./wire.ts";
import { callNative, forgetNative, isNative, isNativeId, loadNative, type NativeLanguage } from "./native.ts";

// ── registration ───────────────────────────────────────────────────────────

/** Every language handed to the module, by the id the module hands back
 *  in `fig_host.call`. A language is never unregistered — the core holds
 *  its format for the life of the process — so nothing is ever removed
 *  from here except an entry whose registration the core refused. */
const languages = new Map<number, Language>();
let nextId = 1;
/** Each language registered, and the format it was given. */
const registered = new Map<Language | NativeLanguage, Format>();

/** The `fig_host.call` import: read the request line out of linear memory,
 *  answer it with {@link handle}, and hand the response back in memory the
 *  module frees. Called from inside a wasm call — a parse, or the harness
 *  at registration — so `fig_alloc` here re-enters the module, which
 *  WebAssembly allows and the allocator does not mind; the views are
 *  re-derived afterwards because the allocation may have grown memory. */
function hostCall(lang: number, requestPtr: number, requestLen: number, outPtr: number, outLen: number): number {
  let bytes: Uint8Array;
  if (isNativeId(lang)) {
    const answer = callNative(lang, u8().slice(requestPtr, requestPtr + requestLen));
    if (!answer) return 1;
    bytes = answer;
  } else {
    const language = languages.get(lang);
    if (!language) return 1;
    const request = decoder.decode(u8().subarray(requestPtr, requestPtr + requestLen));
    bytes = encoder.encode(handle(language, request));
  }
  const ptr = fig.fig_alloc(bytes.length);
  if (ptr === 0) return 1;
  u8().set(bytes, ptr);
  const view = dv();
  view.setUint32(outPtr, ptr, true);
  view.setUint32(outLen, bytes.length, true);
  return 0;
}
setHostCall(hostCall);

/** Register `lang`, the way a caller brings a format into the module: the
 *  package compiles none in, so `@diaryx/fig-yaml` and its peers
 *  are registered here, each the caller imports, and the rest cost nothing.
 *  Returns the `Format` of the language's first dialect, and from then on
 *  `lang.format` is that `Format` too (`yaml.format`); the other dialects
 *  — JSON5's `jsonc`, YAML's `yaml-1.1` — are found by name with
 *  {@link formatByName}. `Document.parse`, `Editor.open`, `stringify`,
 *  `capabilities` and every other call that takes a format accept them at
 *  the tier `lang.caps` declares.
 *
 *  Registering the same object again returns the format it already has, so
 *  a library and the application using it may each register what they
 *  use. Registration validates the description and runs fig's harness over
 *  `samples` — each parsed, printed, reparsed and edited — and throws a
 *  {@link FigError} with the reason when either fails; nothing is
 *  registered then. A name already registered is refused the same way.
 *
 *  Formats are handles assigned per process in registration order, from 1:
 *  persist a format's NAME and resolve it with {@link formatByName}. */
export function registerLanguage(lang: Language | NativeLanguage): Format {
  // The same language again — a library and the application using it each
  // registering the YAML they import — is the format it already is.
  const known = registered.get(lang);
  if (known !== undefined) return known;
  const id = nextId++;
  const native = isNative(lang);
  if (native) loadNative(id, lang);
  else languages.set(id, lang);
  const frame = new Frame();
  try {
    const outFormat = frame.alloc(4);
    const err = allocFigError(frame);
    const status = native ? fig.fig_host_native_register(id, outFormat, err) : fig.fig_host_language_register(id, outFormat, err);
    if (status !== Status.Ok) {
      languages.delete(id);
      forgetNative(id);
      const detail = readFigError(err);
      throw new FigError(status, `registerLanguage(${lang.name})`, { message: detail.message, byteOffset: detail.byteOffset });
    }
    const format = readU32(outFormat) as Format;
    registered.set(lang, format);
    // `yaml.format`: written on the language object, not enumerable, so
    // that a spread of it (`{ ...yaml, name: "x" }`, a new language) does
    // not carry this one's format along. A frozen object keeps none; the
    // returned format is the same.
    if (Object.isExtensible(lang) && !Object.hasOwn(lang, "format")) {
      Object.defineProperty(lang, "format", { value: format, enumerable: false, writable: false, configurable: true });
    }
    return format;
  } finally {
    frame.dispose();
  }
}

/** Content sniffing in the caller's order: the first format `order` names
 *  whose parser accepts `input`, or `null`. The grammars overlap, so put
 *  the strictest first; a name nothing answers to is skipped. */
export function detectFormat(input: string, order: readonly string[]): Format | null {
  const frame = new Frame();
  try {
    const src = frame.str(input);
    const names = order.map((n) => frame.str(n + "\0").ptr);
    const list = frame.alloc(4 * Math.max(names.length, 1));
    const out = frame.alloc(4);
    const view = dv();
    names.forEach((p, i) => view.setUint32(list + 4 * i, p, true));
    const status = fig.fig_detect_order(src.ptr, src.len, list, names.length, out);
    return status === 0 ? (dv().getInt32(out, true) as Format) : null;
  } finally {
    frame.dispose();
  }
}

/** One document of a multi-document stream, from {@link splitStream}: its
 *  text, its byte range `[start, end)` of the input's UTF-8, and whether it
 *  opened with a document-start marker (YAML's `---`). */
export interface StreamDocument {
  text: string;
  start: number;
  end: number;
  explicit: boolean;
}

/** The documents of a multi-document stream in `format` (YAML's
 *  `---`/`...`), each one's text to parse as a document of its own. An empty
 *  stream is one document. Throws a {@link FigError} with
 *  `Status.UnsupportedOperation` for a format without streams. */
export function splitStream(input: string | Uint8Array, format: Format): StreamDocument[] {
  const bytes = typeof input === "string" ? encoder.encode(input) : input;
  const frame = new Frame();
  try {
    const src = frame.bytes(bytes);
    const outDocs = frame.alloc(4);
    const outCount = frame.alloc(4);
    const errPtr = allocFigError(frame);
    const status = fig.fig_stream_split(src, bytes.length, format, outDocs, outCount, errPtr);
    if (status !== Status.Ok) {
      const e = readFigError(errPtr);
      throw new FigError(status, "fig_stream_split", { message: e.message, byteOffset: e.byteOffset });
    }
    const docs = readU32(outDocs);
    const count = readU32(outCount);
    // FigStreamDoc on wasm32: start u32, end u32, explicit bool, padded to 12.
    const out: StreamDocument[] = [];
    const view = dv();
    for (let i = 0; i < count; i++) {
      const at = docs + 12 * i;
      const start = view.getUint32(at, true);
      const end = view.getUint32(at + 4, true);
      out.push({ text: decoder.decode(bytes.subarray(start, end)), start, end, explicit: view.getUint8(at + 8) !== 0 });
    }
    fig.fig_stream_free(docs, count);
    return out;
  } finally {
    frame.dispose();
  }
}
