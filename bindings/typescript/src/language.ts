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

// ── registration ───────────────────────────────────────────────────────────

/** Every language handed to the module, by the id the module hands back
 *  in `fig_host.call`. A language is never unregistered — the core holds
 *  its format for the life of the process — so nothing is ever removed
 *  from here except an entry whose registration the core refused. */
const languages = new Map<number, Language>();
let nextId = 1;

/** The `fig_host.call` import: read the request line out of linear memory,
 *  answer it with {@link handle}, and hand the response back in memory the
 *  module frees. Called from inside a wasm call — a parse, or the harness
 *  at registration — so `fig_alloc` here re-enters the module, which
 *  WebAssembly allows and the allocator does not mind; the views are
 *  re-derived afterwards because the allocation may have grown memory. */
function hostCall(lang: number, requestPtr: number, requestLen: number, outPtr: number, outLen: number): number {
  const language = languages.get(lang);
  const request = decoder.decode(u8().subarray(requestPtr, requestPtr + requestLen));
  const response = language ? handle(language, request) : JSON.stringify({ ok: false, message: `no language registered as ${lang}` });
  const bytes = encoder.encode(response);
  const ptr = fig.fig_alloc(bytes.length);
  if (ptr === 0) return 1;
  u8().set(bytes, ptr);
  const view = dv();
  view.setUint32(outPtr, ptr, true);
  view.setUint32(outLen, bytes.length, true);
  return 0;
}
setHostCall(hostCall);

/** Register `lang`. The returned `Format` is a peer of the compiled ones at
 *  the tier `lang.caps` declares: `Document.parse`, `Editor.open`,
 *  `serialize`, `capabilities` and every other call that takes a format
 *  accept it from then on. Registration validates the description by the
 *  rules a compiled format is held to and runs fig's harness over
 *  `samples` — each parsed, printed, reparsed and edited — and throws a
 *  {@link FigError} with the reason when either fails; nothing is
 *  registered then. A name already taken, compiled or registered, is
 *  refused the same way.
 *
 *  The format integer is assigned per process; persist the NAME and
 *  resolve it with {@link formatByName}. */
export function registerLanguage(lang: Language): Format {
  const id = nextId++;
  languages.set(id, lang);
  const frame = new Frame();
  try {
    const outFormat = frame.alloc(4);
    const err = allocFigError(frame);
    const status = fig.fig_host_language_register(id, outFormat, err);
    if (status !== Status.Ok) {
      languages.delete(id);
      const detail = readFigError(err);
      throw new FigError(status, `registerLanguage(${lang.name})`, { message: detail.message });
    }
    return readU32(outFormat) as Format;
  } finally {
    frame.dispose();
  }
}

/** The `Format` of the dialect named `name` — a compiled format's registry
 *  name (`"json5"`, `"yaml"`, …) or a registered language's — or `null`.
 *  The only stable way to persist a runtime format: its integer is assigned
 *  per process, its name is not. */
export function formatByName(name: string): Format | null {
  const frame = new Frame();
  try {
    const s = frame.str(name + "\0");
    const value = fig.fig_format_by_name(s.ptr);
    return value < 0 ? null : (value as Format);
  } finally {
    frame.dispose();
  }
}
