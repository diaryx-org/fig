// `import * as N from "@diaryx/fig/kit/number"`: how a number's lexeme is
// spelled for a format — the Zig kit's `number.zig`, answering through its
// WebAssembly module (`zig.js`), so a language written here spells a
// number exactly as a Zig language does.
//
// A number row's `text` is its source lexeme, verbatim: `0xff`, `1_000`,
// `.5`, `+7`. A printer's instinct is to write it back as it stands, and
// that is right only when the format being written reads the lexeme back
// as the same number. It often does not: YAML 1.2 reads `0b1010` as a
// string, strict JSON refuses `.5` outright. So a printer asks
// `spellable(raw, spelling)` first and falls back to `canonical(raw)` —
// decimal, the one spelling every format shares. The value is unchanged;
// only its notation degrades.

import * as Z from "./zig.js";

/** The spellings a format reads back as a number. A field left `false`
 *  means that spelling is canonicalized to decimal before it is written:
 *  `hex` (`0x1F`), `octal` (`0o17`), `binary` (`0b1010`), `underscores`
 *  (`1_000`), `leadingZero` (`0755`), `bareDot` (`.5`, `5.`), `plus` (a
 *  leading `+`). */
export const NONE = Object.freeze({});

/** Strict JSON: none of it. */
export const JSON = NONE;

/** JSON5's numbers are ES5.1 numeric literals: hex, a leading `+`, and
 *  bare dots — not `0o`/`0b`, `_`, or a leading zero. */
export const JSON5 = Object.freeze({ hex: true, bareDot: true, plus: true });

/** YAML 1.2 core: hex and octal, a bare dot, a leading `+`, and a leading
 *  zero (read as decimal); no `0b` and no `_`, which 1.2 reads as strings. */
export const YAML_1_2 = Object.freeze({ hex: true, octal: true, leadingZero: true, bareDot: true, plus: true });

/** Whether a format that spells `s` reads `raw` back as the same number. */
export function spellable(raw, s) {
  return Z.numberSpellable(raw, s ?? NONE);
}

/** `raw` as the decimal lexeme every format reads: a radix converted, `_`
 *  and a leading `+` dropped, a bare dot padded, leading zeros stripped.
 *  A lexeme that is not a number at all comes back as it was. */
export function canonical(raw) {
  return Z.numberCanonical(raw);
}

/** Which non-finite float `raw` spells, in any format's words (`inf`,
 *  `.inf`, `Infinity`, `nan`, …, with an optional sign): "inf", "-inf",
 *  "nan", or null when it is a finite number or none. */
export function nonFinite(raw) {
  return Z.numberNonFinite(raw);
}

/** `raw` as a format that spells `s` writes it: itself when spellable,
 *  else canonical. */
export function text(raw, s) {
  return spellable(raw, s) ? raw : canonical(raw);
}

export default { NONE, JSON, JSON5, YAML_1_2, spellable, canonical, nonFinite, text };
