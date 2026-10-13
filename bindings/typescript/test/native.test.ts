// The languages compiled from fig's own Zig, each a WebAssembly module of
// its own (`@diaryx/fig-<name>`, `src/native.ts`): registered, they
// parse and print through their module with the node table crossing as
// its rows, and describe, render and split over the helper wire.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Document, Editor, FigError, convert, formatByName, init, registerLanguage, splitStream } from "../src/index.ts";
import yaml from "@diaryx/fig-yaml";
import json from "@diaryx/fig-json";
import dotenv from "@diaryx/fig-dotenv";
import toml from "@diaryx/fig-toml";

await init(yaml, json, dotenv, toml);
const Yaml = registerLanguage(yaml);
const Json = registerLanguage(json);
const Dotenv = registerLanguage(dotenv);
registerLanguage(toml);

test("a native language registers once, with every dialect its module serves", () => {
  assert.equal(registerLanguage(yaml), Yaml);
  assert.equal(yaml.format, Yaml);
  assert.notEqual(formatByName("yaml-1.1"), null);
  assert.notEqual(formatByName("jsonc"), null);
  assert.notEqual(formatByName("json5"), null);
});

test("a spec version is a dialect of its own, parsing at that spec", () => {
  const parsed = (src: string, name: string) => {
    const format = formatByName(name);
    assert.notEqual(format, null, name);
    const doc = Document.parse(src, format!);
    try {
      return doc.get(["a"]);
    } finally {
      doc.dispose();
    }
  };
  // YAML 1.1 resolves `yes` as a boolean; 1.2.2, the default, as a string.
  assert.equal(parsed("a: yes\n", "yaml"), "yes");
  assert.equal(parsed("a: yes\n", "yaml-1.1"), true);
  // A trailing comma in an inline table is TOML 1.1's; 1.0 refuses it.
  assert.deepEqual(parsed("a = { b = 1, }\n", "toml"), { b: 1 });
  assert.deepEqual(parsed("a = { b = 1, }\n", "toml-1.1"), { b: 1 });
  assert.throws(() => parsed("a = { b = 1, }\n", "toml-1.0"), FigError);
  assert.equal(parsed("a = 1\n", "toml-1.0.0"), 1);
});

test("the table crosses both ways: parse, read, print", () => {
  const src = "# top\nname: fig # trailing\nlist:\n  - &a 1\n  - *a\nnested: {x: [1, 2], y: q}\n";
  const doc = Document.parse(src, Yaml);
  try {
    assert.equal(doc.get(["name"]), "fig");
    assert.deepEqual(doc.get(["nested", "x"]), [1, 2]);
    assert.equal(doc.serialize(Yaml), "# top\nname: fig # trailing\nlist:\n- &a 1\n- *a\nnested: { x: [1, 2], y: q }\n");
  } finally {
    doc.dispose();
  }
  assert.equal(convert("{\"a\": [1, true, null]}", Json, Yaml), "a: [1, true, null]\n");
});

test("an edit through a native language splices and reparses", () => {
  using ed = Editor.open("a: 1 # keep\nb: [x, y]\n", Yaml);
  ed.replaceValue(["a"], 2);
  assert.equal(ed.source(), "a: 2 # keep\nb: [x, y]\n");
});

test("a refusal says why and where", () => {
  assert.throws(
    () => Document.parse("a: [1, 2\n", Yaml),
    (e: unknown) => e instanceof FigError && typeof e.byteOffset === "number",
  );
});

test("a stream splits through the language's module", () => {
  const docs = splitStream("a: 1\n---\nb: 2\n", Yaml);
  assert.deepEqual(docs.map((d) => d.text), ["a: 1\n", "---\nb: 2\n"]);
  assert.deepEqual(docs.map((d) => d.explicit), [false, true]);
});

test("an empty print crosses as an empty string", () => {
  using ed = Editor.open("A=1\n", Dotenv);
  ed.replaceValue(["A"], {});
  assert.equal(ed.source(), "A=\n");
});
