// `import * as DT from "@diaryx/fig/kit/datetime"`: which datetime shape a
// bare token is — the twin of fig's `src/util/datetime.zig`.
//
// TOML, YAML and fig all read `2024-01-02`, `12:30:00`, `2024-01-02T12:30:00`
// and `2024-01-02T12:30:00+01:00` as typed scalars whose `ext_kind` is one
// of `local_date`, `local_time`, `local_datetime` and `offset_datetime`,
// and validate them the same way: a real calendar date, a time of day
// with optional fraction, an offset within a day. What differs per format
// is only what is allowed — whether seconds may be omitted, and whether a
// bare time with no date is a time at all — and that is `opts`.

const isDigit = (c) => c >= 48 && c <= 57;
const two = (s, at) => parseInt(s.slice(at, at + 2), 10);
const bothDigits = (s, at) => at + 1 < s.length && isDigit(s.charCodeAt(at)) && isDigit(s.charCodeAt(at + 1));

function daysInMonth(year, month) {
  if (month === 2) return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0 ? 29 : 28;
  if (month === 4 || month === 6 || month === 9 || month === 11) return 30;
  return 31;
}

/** Whether `s` is `YYYY-MM-DD` and a day that exists. */
export function validDate(s) {
  if (s.length !== 10 || s[4] !== "-" || s[7] !== "-") return false;
  if (!(bothDigits(s, 0) && bothDigits(s, 2) && bothDigits(s, 5) && bothDigits(s, 8))) return false;
  const year = two(s, 0) * 100 + two(s, 2);
  const month = two(s, 5);
  const day = two(s, 8);
  if (month < 1 || month > 12) return false;
  return day >= 1 && day <= daysInMonth(year, month);
}

/** Whether `s` is `HH:MM:SS[.frac]` — or `HH:MM`, when
 *  `opts.minutePrecision` (true) allows the seconds to be omitted. A
 *  second of 60 is allowed, for a leap second. */
export function validTime(s, opts) {
  const minutePrecision = opts?.minutePrecision !== false;
  if (s.length < 5 || s[2] !== ":") return false;
  if (!(bothDigits(s, 0) && bothDigits(s, 3))) return false;
  if (two(s, 0) > 23 || two(s, 3) > 59) return false;
  if (s.length === 5) return minutePrecision;
  if (s[5] !== ":" || s.length < 8) return false;
  if (!bothDigits(s, 6) || two(s, 6) > 60) return false;
  if (s.length === 8) return true;
  if (s[8] !== "." || s.length < 10) return false;
  return /^[0-9]+$/.test(s.slice(9));
}

/** Whether `s` is `±HH:MM`, an offset within a day. */
export function validOffset(s) {
  if (s.length !== 6 || s[3] !== ":") return false;
  if (!(bothDigits(s, 1) && bothDigits(s, 4))) return false;
  return two(s, 1) <= 23 && two(s, 4) <= 59;
}

/** The shape of `raw` — "offset_datetime", "local_datetime", "local_date"
 *  or "local_time" — or null when it is not a datetime. `opts`:
 *  `minutePrecision` (true) accepts `HH:MM` with no seconds, as TOML 1.1
 *  and YAML do and TOML 1.0 does not; `timeOnly` (true) accepts a bare
 *  time with no date, as TOML does and YAML 1.1 does not (there a `:`-run
 *  with no date is a sexagesimal number). The date and the time are joined
 *  by `T`, `t` or a space; an offset is `Z`, `z` or `±HH:MM`. */
export function classify(raw, opts) {
  const timeOnly = opts?.timeOnly !== false;
  if (timeOnly && raw.length >= 3 && raw[2] === ":") return validTime(raw, opts) ? "local_time" : null;
  if (raw.length < 10) return null;
  if (!validDate(raw.slice(0, 10))) return null;
  if (raw.length === 10) return "local_date";
  const sep = raw[10];
  if (sep !== "T" && sep !== "t" && sep !== " ") return null;
  const rest = raw.slice(11);
  let time = rest;
  let hasOffset = false;
  const last = rest[rest.length - 1];
  if (last === "Z" || last === "z") {
    time = rest.slice(0, -1);
    hasOffset = true;
  } else if (rest.length >= 6 && (rest[rest.length - 6] === "+" || rest[rest.length - 6] === "-") && rest[rest.length - 3] === ":") {
    if (!validOffset(rest.slice(-6))) return null;
    time = rest.slice(0, -6);
    hasOffset = true;
  }
  if (!validTime(time, opts)) return null;
  return hasOffset ? "offset_datetime" : "local_datetime";
}

/** The four kinds `classify` answers, for a printer deciding whether an
 *  `ext_kind` is a datetime. */
export const KINDS = new Set(["offset_datetime", "local_datetime", "local_date", "local_time"]);

export default { validDate, validTime, validOffset, classify, KINDS };
