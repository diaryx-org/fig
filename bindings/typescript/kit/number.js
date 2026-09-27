// `import * as N from "@diaryx/fig/kit/number"`: how a number's lexeme is
// spelled for a format — the twin of fig's `src/util/number.zig`.
//
// A number row's `text` is its source lexeme, verbatim: `0xff`, `1_000`,
// `.5`, `+7`. A printer's instinct is to write it back as it stands, and
// that is right only when the format being written reads the lexeme back
// as the same number. It often does not: YAML 1.2 reads `0b1010` as a
// string, strict JSON refuses `.5` outright. So a printer asks
// `spellable(raw, spelling)` first and falls back to `canonical(raw)` —
// decimal, the one spelling every format shares. The value is unchanged;
// only its notation degrades.

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
  s ??= NONE;
  let body = raw;
  const first = raw[0];
  if (first === "+" || first === "-") {
    if (first === "+" && !s.plus) return false;
    body = raw.slice(1);
  }
  if (body === "") return false;
  if (!s.underscores && body.includes("_")) return false;
  if (body.length >= 2 && body[0] === "0") {
    const r = body[1].toLowerCase();
    if (r === "x") return s.hex === true;
    if (r === "o") return s.octal === true;
    if (r === "b") return s.binary === true;
  }
  if (!s.bareDot && (body[0] === "." || body[body.length - 1] === ".")) return false;
  if (!s.leadingZero) {
    const m = body.search(/[.eE]/);
    const intEnd = m < 0 ? body.length : m;
    if (intEnd > 1 && body[0] === "0") return false;
  }
  return true;
}

const RADIX = { x: 16, o: 8, b: 2 };
const DIGITS = { 16: /^[0-9a-fA-F]+$/, 8: /^[0-7]+$/, 2: /^[01]+$/ };

/** `raw` as the decimal lexeme every format reads: a radix converted, `_`
 *  and a leading `+` dropped, a bare dot padded, leading zeros stripped.
 *  A lexeme that is not a number at all comes back as it was. */
export function canonical(raw) {
  let out = "";
  let s = raw;
  if (s[0] === "-") {
    out = "-";
    s = s.slice(1);
  } else if (s[0] === "+") {
    s = s.slice(1);
  }
  if (s.length >= 2 && s[0] === "0" && Object.hasOwn(RADIX, s[1].toLowerCase())) {
    const base = RADIX[s[1].toLowerCase()];
    const digits = s.slice(2).replace(/_/g, "");
    if (digits === "" || !DIGITS[base].test(digits)) return out + s;
    let v = 0n;
    for (const d of digits) v = v * BigInt(base) + BigInt(parseInt(d, 16));
    return out + v.toString();
  }
  const eIdx = s.search(/[eE]/);
  const mantissa = eIdx < 0 ? s : s.slice(0, eIdx);
  const exponent = eIdx < 0 ? "" : s.slice(eIdx);
  const dot = mantissa.indexOf(".");
  const intPart = dot < 0 ? mantissa : mantissa.slice(0, dot);
  const intDigits = intPart.replace(/_/g, "").replace(/^0+/, "");
  out += intDigits === "" ? "0" : intDigits;
  if (dot >= 0) {
    const frac = mantissa.slice(dot + 1).replace(/_/g, "");
    out += "." + (frac === "" ? "0" : frac);
  }
  return out + exponent.replace(/_/g, "");
}

/** `raw` as a format that spells `s` writes it: itself when spellable,
 *  else canonical. */
export function text(raw, s) {
  return spellable(raw, s) ? raw : canonical(raw);
}

export default { NONE, JSON, JSON5, YAML_1_2, spellable, canonical, text };
