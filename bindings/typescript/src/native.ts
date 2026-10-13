// Languages compiled from fig's own Zig, each a WebAssembly module of its
// own beside the core's: `@diaryx/fig/languages/<name>`. The core reaches
// one the way it reaches a language written in JavaScript — described,
// and asked for renderings and stream splits, over the helper wire — but
// parses and prints through it as the node table's rows: a parse writes
// its table in the language's memory, and this copies each list of it into
// the table the core handed the parse, byte for byte, since every offset
// in it is into the input or the table's own buffer, which both sides
// hold. A print copies the other way. See `src/kit/wasm_language.zig` in
// the core.
import { decodeBase64, dv, fig, init as initCore, setHostNative, u8, FIG_ERROR_SIZE } from "./ffi.ts";
import type { Format } from "./types.ts";

/** A language compiled from fig's Zig: its name and its module. Pass it to
 *  {@link registerLanguage}, as any language. */
export interface NativeLanguage {
  readonly name: string;
  /** The module, base64. */
  readonly wasm: string;
  /** Set by {@link registerLanguage}: the format of its first dialect. */
  readonly format?: Format;
}

export function isNative(lang: object): lang is NativeLanguage {
  return typeof (lang as NativeLanguage).wasm === "string";
}

/** What a language module exports (`kit/wasm_language.zig`). */
interface ModuleExports {
  memory: WebAssembly.Memory;
  fig_lang_alloc(n: number): number;
  fig_lang_free(ptr: number, n: number): void;
  fig_lang_parse(dialect: number, dialectLen: number, input: number, inputLen: number, view: number, err: number): number;
  fig_lang_print(dialect: number, dialectLen: number, view: number, options: number, out: number, err: number): number;
  fig_lang_call(request: number, requestLen: number, out: number): number;
  fig_lang_view_size(): number;
}

/** Each list of a table, in the order `fig_table_extend` numbers them,
 *  by its element's size in bytes — `list_sizes` in the core. */
const LIST_SIZES: readonly number[] = [32, 1, 20, 24, 12, 12, 12, 16, 12, 16, 16, 28];
const sizeOf = (list: number): number => LIST_SIZES[list] ?? 1;
/** `FigTableView` on wasm32: `size`, then source, strings and each list as
 *  a pointer and a count. */
const VIEW_SIZE = 108;
const VIEW_LISTS = 2; // the rows are the view's third pair; source is the first, strings the second

/** The module of each registered native language, by the id the core
 *  calls it by. */
const modules = new Map<number, ModuleExports>();

/** Each language's instance made ahead by {@link init}, for
 *  `registerLanguage` to take rather than compile on the spot. */
const prepared = new WeakMap<NativeLanguage, ModuleExports>();

/** Instantiate `lang`'s module — the one {@link init} made, else
 *  synchronously now — and keep it as `id`'s. */
export function loadNative(id: number, lang: NativeLanguage): void {
  let m = prepared.get(lang);
  if (!m) {
    const instance = new WebAssembly.Instance(new WebAssembly.Module(decodeBase64(lang.wasm)), {});
    m = instance.exports as unknown as ModuleExports;
  }
  modules.set(id, m);
}

/** Compile and instantiate the fig module, and each of `languages`'
 *  modules, asynchronously — what a browser's main thread must do before
 *  using fig, since it may not compile a module of more than 4 KB
 *  synchronously. Then `registerLanguage` and every other call is
 *  synchronous. Optional under Node and in Web Workers, where each module
 *  compiles on first use. Idempotent. */
export async function init(...languages: NativeLanguage[]): Promise<void> {
  await Promise.all([
    initCore(),
    ...languages.filter((l) => !prepared.has(l)).map(async (l) => {
      const { instance } = await WebAssembly.instantiate(decodeBase64(l.wasm), {});
      prepared.set(l, instance.exports as unknown as ModuleExports);
    }),
  ]);
}

export function forgetNative(id: number): void {
  modules.delete(id);
}

export function isNativeId(id: number): boolean {
  return modules.has(id);
}

/** Answer a wire request line for native language `id`: the module's
 *  `fig_lang_call`. Null when it cannot answer. */
export function callNative(id: number, request: Uint8Array): Uint8Array | null {
  const m = modules.get(id);
  if (!m) return null;
  const req = m.fig_lang_alloc(request.length);
  const out = m.fig_lang_alloc(8);
  try {
    new Uint8Array(m.memory.buffer).set(request, req);
    if (m.fig_lang_call(req, request.length, out) !== 0) return null;
    const mv = new DataView(m.memory.buffer);
    const ptr = mv.getUint32(out, true);
    const len = mv.getUint32(out + 4, true);
    if (len === 0) return new Uint8Array();
    const bytes = new Uint8Array(m.memory.buffer, ptr, len).slice();
    m.fig_lang_free(ptr, len);
    return bytes;
  } finally {
    m.fig_lang_free(req, request.length);
    m.fig_lang_free(out, 8);
  }
}

/** Copy `len` bytes at `from` in the core's memory into a fresh allocation
 *  in `m`'s. */
function toModule(m: ModuleExports, from: number, len: number): number {
  const at = m.fig_lang_alloc(len);
  if (len > 0) new Uint8Array(m.memory.buffer).set(u8().subarray(from, from + len), at);
  return at;
}

/** `fig_host.parse`: parse in the module, then copy its table into the
 *  core's `table` list by list, and its error record into `err`. */
function parse(id: number, dialect: number, dialectLen: number, input: number, inputLen: number, table: number, err: number): number {
  const m = modules.get(id);
  if (!m) return 4;
  const d = toModule(m, dialect, dialectLen);
  const src = toModule(m, input, inputLen);
  const view = m.fig_lang_alloc(VIEW_SIZE);
  const e = m.fig_lang_alloc(FIG_ERROR_SIZE);
  try {
    new Uint8Array(m.memory.buffer).fill(0, e, e + FIG_ERROR_SIZE);
    new DataView(m.memory.buffer).setUint32(e, FIG_ERROR_SIZE, true);
    const rc = m.fig_lang_parse(d, dialectLen, src, inputLen, view, e);
    const mv = new DataView(m.memory.buffer);
    // The buffer (list 1) is the view's second pair; the rest follow it
    // from the third.
    for (let list = 0; list < LIST_SIZES.length; list++) {
      const pair = list === 0 ? VIEW_LISTS : list === 1 ? 1 : VIEW_LISTS + list - 1;
      const ptr = mv.getUint32(view + 4 + 8 * pair, true);
      const count = mv.getUint32(view + 4 + 8 * pair + 4, true);
      if (count === 0) continue;
      const at = fig.fig_table_extend(table, list, count);
      if (at === 0) return 3;
      const bytes = count * sizeOf(list);
      u8().set(new Uint8Array(m.memory.buffer, ptr, bytes), at);
    }
    // The error record after its `size`, which is the core's.
    u8().set(new Uint8Array(m.memory.buffer, e + 4, FIG_ERROR_SIZE - 4), err + 4);
    return rc;
  } finally {
    m.fig_lang_free(d, dialectLen);
    m.fig_lang_free(src, inputLen);
    m.fig_lang_free(view, VIEW_SIZE);
    m.fig_lang_free(e, FIG_ERROR_SIZE);
  }
}

/** `fig_host.print`: copy the table `view` shows into the module, print it
 *  there, and hand the text back in core memory. */
function print(id: number, dialect: number, dialectLen: number, view: number, options: number, out: number, err: number): number {
  const m = modules.get(id);
  if (!m) return 4;
  const owned: Array<[number, number]> = [];
  const own = (at: number, len: number): number => {
    owned.push([at, len]);
    return at;
  };
  try {
    const d = own(toModule(m, dialect, dialectLen), dialectLen);
    const cv = dv();
    const mview = own(m.fig_lang_alloc(VIEW_SIZE), VIEW_SIZE);
    const pairs: Array<[number, number]> = [];
    // Source, buffer, then the lists, each copied whole.
    for (let pair = 0; pair < (VIEW_SIZE - 4) / 8; pair++) {
      const ptr = cv.getUint32(view + 4 + 8 * pair, true);
      const count = cv.getUint32(view + 4 + 8 * pair + 4, true);
      const size = pair === 0 || pair === 1 ? 1 : sizeOf(pair === VIEW_LISTS ? 0 : pair - 1);
      pairs.push([count === 0 ? 0 : own(toModule(m, ptr, count * size), count * size), count]);
    }
    const mv = new DataView(m.memory.buffer);
    mv.setUint32(mview, VIEW_SIZE, true);
    pairs.forEach(([ptr, count], pair) => {
      mv.setUint32(mview + 4 + 8 * pair, ptr, true);
      mv.setUint32(mview + 4 + 8 * pair + 4, count, true);
    });
    const optionsSize = cv.getUint32(options, true);
    const o = own(toModule(m, options, optionsSize), optionsSize);
    const mout = own(m.fig_lang_alloc(8), 8);
    const e = own(m.fig_lang_alloc(FIG_ERROR_SIZE), FIG_ERROR_SIZE);
    new Uint8Array(m.memory.buffer).fill(0, e, e + FIG_ERROR_SIZE);
    new DataView(m.memory.buffer).setUint32(e, FIG_ERROR_SIZE, true);
    const rc = m.fig_lang_print(d, dialectLen, mview, o, mout, e);
    const after = new DataView(m.memory.buffer);
    if (rc === 0) {
      const ptr = after.getUint32(mout, true);
      const len = after.getUint32(mout + 4, true);
      const at = fig.fig_alloc(Math.max(len, 1));
      if (at === 0) return 3;
      // An empty text's pointer is no address at all: nothing to copy or
      // free.
      if (len > 0) {
        u8().set(new Uint8Array(m.memory.buffer, ptr, len), at);
        m.fig_lang_free(ptr, len);
      }
      dv().setUint32(out, at, true);
      dv().setUint32(out + 4, len, true);
    }
    u8().set(new Uint8Array(m.memory.buffer, e + 4, FIG_ERROR_SIZE - 4), err + 4);
    return rc;
  } finally {
    for (const [at, len] of owned) m.fig_lang_free(at, len);
  }
}

setHostNative({ parse, print });
