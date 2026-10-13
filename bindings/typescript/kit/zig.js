// The Zig kit's rules, for the JavaScript kit: `fig-kit.wasm` (the fig
// repository's `kit/src/js.zig`, whose functions `fig.h` declares as
// `fig_kit_*`), inlined as base64 by scripts/build-wasm.mjs into
// `zig-bytes.js` and compiled synchronously the first time a rule is asked. At under 5 KB it is far inside what a
// browser's main thread compiles synchronously.
//
// `number.js` and `datetime.js` are written on these functions, and
// nothing else here touches the module, so a host with no WebAssembly
// (fig-quickjs) answers the same functions from `libfig`'s `fig_kit_*` by
// replacing this file alone.
import { BYTES } from "./zig-bytes.js";

let kit = null;

function exports() {
  if (kit) return kit;
  const bytes = typeof Buffer === "function" ? Buffer.from(BYTES, "base64") : Uint8Array.from(atob(BYTES), (c) => c.charCodeAt(0));
  kit = new WebAssembly.Instance(new WebAssembly.Module(bytes), {}).exports;
  return kit;
}

// How much longer than its input an answer can be (`FIG_KIT_ANSWER_ROOM`).
const ANSWER_ROOM = 520;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** `s`'s UTF-8 written at the heap's start, with room behind it for an
 *  answer: [where, how long]. */
function put(s) {
  const k = exports();
  const at = k.fig_kit_heap();
  const most = s.length * 3;
  const need = at + 2 * most + ANSWER_ROOM;
  const have = k.memory.buffer.byteLength;
  if (need > have) k.memory.grow(Math.ceil((need - have) / 65536));
  const mem = new Uint8Array(k.memory.buffer, at, most);
  let n = 0;
  for (; n < s.length; n++) {
    const c = s.charCodeAt(n);
    if (c >= 0x80) return [at, encoder.encodeInto(s, mem).written];
    mem[n] = c;
  }
  return [at, n];
}

/** A number spelling (`number.js`) as `fig.h`'s `FIG_SPELL_*` bits. */
function spellingBits(s) {
  return (s.hex ? 1 : 0) | (s.octal ? 2 : 0) | (s.binary ? 4 : 0) | (s.underscores ? 8 : 0) | (s.leadingZero ? 16 : 0) | (s.bareDot ? 32 : 0) | (s.plus ? 64 : 0);
}

/** Datetime options (`datetime.js`) as `fig.h`'s `FIG_DATETIME_*` bits;
 *  both default on. */
function optionBits(opts) {
  return (opts?.minutePrecision !== false ? 1 : 0) | (opts?.timeOnly !== false ? 2 : 0);
}

export function numberSpellable(raw, s) {
  const [at, len] = put(raw);
  return exports().fig_kit_number_spellable(at, len, spellingBits(s)) !== 0;
}

export function numberCanonical(raw) {
  const [at, len] = put(raw);
  const k = exports();
  const n = k.fig_kit_number_canonical(at, len, at + len, len + ANSWER_ROOM);
  return decoder.decode(new Uint8Array(k.memory.buffer, at + len, n));
}

const NON_FINITE = ["inf", "-inf", "nan"];

export function numberNonFinite(raw) {
  const [at, len] = put(raw);
  return NON_FINITE[exports().fig_kit_number_non_finite(at, len)] ?? null;
}

const KINDS = ["offset_datetime", "local_datetime", "local_date", "local_time"];

export function datetimeClassify(raw, opts) {
  const [at, len] = put(raw);
  return KINDS[exports().fig_kit_datetime_classify(at, len, optionBits(opts))] ?? null;
}

export function datetimeValidDate(s) {
  const [at, len] = put(s);
  return exports().fig_kit_datetime_valid_date(at, len) !== 0;
}

export function datetimeValidTime(s, opts) {
  const [at, len] = put(s);
  return exports().fig_kit_datetime_valid_time(at, len, optionBits(opts)) !== 0;
}

export function datetimeValidOffset(s) {
  const [at, len] = put(s);
  return exports().fig_kit_datetime_valid_offset(at, len) !== 0;
}
