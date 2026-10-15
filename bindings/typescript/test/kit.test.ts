// The JavaScript kit's rules, which answer through the Zig kit's
// WebAssembly module (`kit/rules.js`): that the bridge carries each input
// and answer whole, whatever its length or its characters. The rules
// themselves are the Zig kit's, and its own tests hold them.
import { test } from "node:test";
import assert from "node:assert/strict";

import * as N from "@diaryx/fig/kit/number";
import * as DT from "@diaryx/fig/kit/datetime";

test("number: spellable follows the spelling it is given", () => {
  assert.equal(N.spellable("0xff", N.JSON), false);
  assert.equal(N.spellable("0xff", N.JSON5), true);
  assert.equal(N.spellable("0o755", N.YAML_1_2), true);
  assert.equal(N.spellable("1_000", N.YAML_1_2), false);
  assert.equal(N.spellable("+5", undefined), false);
  assert.equal(N.spellable("1.10", N.JSON), true);
});

test("number: canonical converts a radix past 128 bits, and leaves a non-number", () => {
  assert.equal(N.canonical("0xdead_beef"), "3735928559");
  assert.equal(N.canonical("-0b1010"), "-10");
  assert.equal(N.canonical(".5"), "0.5");
  assert.equal(N.canonical("5."), "5.0");
  assert.equal(N.canonical("0x1" + "0".repeat(32)), (2n ** 128n).toString());
  assert.equal(N.canonical("0xg"), "0xg");
  assert.equal(N.text("0xff", N.JSON), "255");
  assert.equal(N.text("0xff", N.JSON5), "0xff");
});

test("number: an answer as long as a long input", () => {
  const digits = "1234567890".repeat(10_000);
  assert.equal(N.canonical("+" + digits + "."), digits + ".0");
  assert.equal(N.canonical("0x" + "f".repeat(400)), (16n ** 400n - 1n).toString());
});

test("number: nonFinite reads each format's words", () => {
  assert.equal(N.nonFinite("-.inf"), "-inf");
  assert.equal(N.nonFinite("Infinity"), "inf");
  assert.equal(N.nonFinite(".NaN"), "nan");
  assert.equal(N.nonFinite("1e999"), null);
});

test("number: text respells a non-finite float in the format's words, or refuses it", () => {
  assert.equal(N.text(".NaN", N.YAML_1_2), ".nan");
  assert.equal(N.text("-inf", N.YAML_1_2), "-.inf");
  assert.equal(N.text("+.inf", N.JSON5), "Infinity");
  assert.equal(N.text("nan", N.JSON5), "NaN");
  assert.equal(N.PLAIN_NON_FINITE[N.nonFinite("-Infinity")!], "-inf");
  assert.throws(() => N.text("nan", N.JSON), { name: "LanguageError", code: 6 });
});

test("datetime: classify and its options", () => {
  assert.equal(DT.classify("1979-05-27T07:32:00Z"), "offset_datetime");
  assert.equal(DT.classify("1979-05-27 07:32:00.999-07:00"), "offset_datetime");
  assert.equal(DT.classify("1979-05-27T07:32:00"), "local_datetime");
  assert.equal(DT.classify("1979-05-27"), "local_date");
  assert.equal(DT.classify("07:32:00"), "local_time");
  assert.equal(DT.classify("07:32:00", { timeOnly: false }), null);
  assert.equal(DT.classify("1979-05-27T07:32", { minutePrecision: false }), null);
  assert.equal(DT.classify("2002-02-30"), null);
  assert.equal(DT.classify("2024-02-29"), "local_date");
  assert.equal(DT.validTime("23:59:60"), true);
  assert.equal(DT.validOffset("+24:00"), false);
});

test("a string that is not ASCII crosses whole", () => {
  assert.equal(DT.classify("1979-05-27T07:32:00Zé"), null);
  assert.equal(N.canonical("1é"), "1é");
  assert.equal(N.spellable("１", N.JSON), true);
});
