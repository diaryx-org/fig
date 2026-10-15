// The JavaScript kit's rules — `/number`, `/datetime`, `/literal` and
// `/json-string`, which answer through the Zig kit's WebAssembly module —
// held to the fixtures every binding's kit is held to
// (`kit/testdata/rules/`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import * as N from "@diaryx/fig/kit/number";
import * as DT from "@diaryx/fig/kit/datetime";
import * as L from "@diaryx/fig/kit/literal";
import * as J from "@diaryx/fig/kit/json-string";

const dir = join(import.meta.dirname, "..", "..", "..", "kit", "testdata", "rules");
const fixture = (name: string) => JSON.parse(readFileSync(join(dir, name), "utf8"));

const SPELLING: Record<string, string> = {
  hex: "hex",
  octal: "octal",
  binary: "binary",
  underscores: "underscores",
  leading_zero: "leadingZero",
  bare_dot: "bareDot",
  plus: "plus",
};

const spelling = (names: string[]) => Object.fromEntries(names.map((n) => [SPELLING[n], true]));
const options = (o: { minute_precision: boolean; time_only: boolean }) => ({ minutePrecision: o.minute_precision, timeOnly: o.time_only });

test("rules: number", () => {
  const fx = fixture("number.json");
  for (const [raw, s, want] of fx.spellable) assert.equal(N.spellable(raw, spelling(s)), want, `spellable ${raw}`);
  for (const [raw, want] of fx.canonical) assert.equal(N.canonical(raw), want, `canonical ${raw}`);
  for (const [raw, want] of fx.non_finite) assert.equal(N.nonFinite(raw), want, `non_finite ${raw}`);
});

test("rules: datetime", () => {
  const fx = fixture("datetime.json");
  for (const [raw, o, want] of fx.classify) assert.equal(DT.classify(raw, options(o)), want, `classify ${raw}`);
  for (const [s, want] of fx.valid_date) assert.equal(DT.validDate(s), want, `valid_date ${s}`);
  for (const [s, o, want] of fx.valid_time) assert.equal(DT.validTime(s, options(o)), want, `valid_time ${s}`);
  for (const [s, want] of fx.valid_offset) assert.equal(DT.validOffset(s), want, `valid_offset ${s}`);
});

test("rules: literal", () => {
  const fx = fixture("literal.json");
  for (const [raw, want] of fx.kind) assert.equal(L.kind(raw), want, `kind ${raw}`);
  for (const [raw, want] of fx.number) assert.equal(L.number(raw), want, `number ${raw}`);
});

test("rules: json-string", () => {
  const fx = fixture("json-string.json");
  for (const [raw, q, want] of fx.quote) assert.equal(J.quote(raw, q), want, `quote ${JSON.stringify(raw)}`);
  assert.equal(J.quote("x".repeat(100_000) + "\u0001"), '"' + "x".repeat(100_000) + '\\u0001"');
  assert.throws(() => J.quote("a", "`"), RangeError);
});
