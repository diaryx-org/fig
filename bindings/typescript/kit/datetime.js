// `import * as DT from "@diaryx/fig/kit/datetime"`: which datetime shape a
// bare token is — the Zig kit's `datetime.zig`, answering through its
// WebAssembly module (`zig.js`).
//
// TOML, YAML and fig all read `2024-01-02`, `12:30:00`, `2024-01-02T12:30:00`
// and `2024-01-02T12:30:00+01:00` as typed scalars whose `ext_kind` is one
// of `local_date`, `local_time`, `local_datetime` and `offset_datetime`,
// and validate them the same way: a real calendar date, a time of day
// with optional fraction, an offset within a day. What differs per format
// is only what is allowed — whether seconds may be omitted, and whether a
// bare time with no date is a time at all — and that is `opts`.

import * as Z from "./zig.js";

/** Whether `s` is `YYYY-MM-DD` and a day that exists. */
export function validDate(s) {
  return Z.datetimeValidDate(s);
}

/** Whether `s` is `HH:MM:SS[.frac]` — or `HH:MM`, when
 *  `opts.minutePrecision` (true) allows the seconds to be omitted. A
 *  second of 60 is allowed, for a leap second. */
export function validTime(s, opts) {
  return Z.datetimeValidTime(s, opts);
}

/** Whether `s` is `±HH:MM`, an offset within a day. */
export function validOffset(s) {
  return Z.datetimeValidOffset(s);
}

/** The shape of `raw` — "offset_datetime", "local_datetime", "local_date"
 *  or "local_time" — or null when it is not a datetime. `opts`:
 *  `minutePrecision` (true) accepts `HH:MM` with no seconds, as TOML 1.1
 *  and YAML do and TOML 1.0 does not; `timeOnly` (true) accepts a bare
 *  time with no date, as TOML does and YAML 1.1 does not (there a `:`-run
 *  with no date is a sexagesimal number). The date and the time are joined
 *  by `T`, `t` or a space; an offset is `Z`, `z` or `±HH:MM`. */
export function classify(raw, opts) {
  return Z.datetimeClassify(raw, opts);
}

/** The four kinds `classify` answers, for a printer deciding whether an
 *  `ext_kind` is a datetime. */
export const KINDS = new Set(["offset_datetime", "local_datetime", "local_date", "local_time"]);

export default { validDate, validTime, validOffset, classify, KINDS };
