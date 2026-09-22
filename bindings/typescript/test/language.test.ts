// Runtime languages: a format written in JavaScript, held to the compiled
// format it twins — table for table before registration, and read, print
// and edit for edit after it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import {
  Document,
  Editor,
  FigError,
  Format,
  Status,
  capabilities,
  convert,
  describe,
  formatByName,
  handle,
  parse,
  registerLanguage,
  serialize,
  serve,
  type Language,
  type NodeTable,
} from "../src/index.ts";
import { dotenv } from "./languages/dotenv.ts";

// The compiled parser's tables, one per fixture: what `fig lang table -i
// dotenv <file>` prints. `npm test` runs from the package directory.
const FIXTURES = join(process.cwd(), "test", "fixtures", "dotenv");
const fixtures = readdirSync(FIXTURES)
  .filter((f) => f.endsWith(".env"))
  .sort()
  .map((f) => ({
    name: f,
    source: readFileSync(join(FIXTURES, f), "utf8"),
    table: JSON.parse(readFileSync(join(FIXTURES, f.replace(/\.env$/, ".table.json")), "utf8")) as NodeTable,
  }));

function parsedThroughWire(lang: Language, input: string): NodeTable {
  const resp = JSON.parse(handle(lang, JSON.stringify({ op: "parse", dialect: lang.name, input }))) as { ok: boolean; table?: NodeTable; message?: string };
  assert.ok(resp.ok, resp.message);
  return resp.table!;
}

test("the JavaScript twin gives the compiled parser's table, row for row", () => {
  assert.ok(fixtures.length >= 10, "fixtures present");
  for (const f of fixtures) {
    assert.deepEqual(parsedThroughWire(dotenv, f.source), f.table, f.name);
  }
});

// Registered once; the format is per process and never unregistered.
const jsDotenv = registerLanguage(dotenv);

test("registerLanguage hands back a runtime format every entry point accepts", () => {
  assert.ok(jsDotenv >= 4096, `runtime formats start at FIG_FORMAT_RUNTIME_BASE, got ${jsDotenv}`);
  assert.deepEqual(capabilities(jsDotenv), { read: true, edit: true, serialize: true, references: false });
  assert.equal(formatByName("js-dotenv"), jsDotenv);
  assert.equal(formatByName("dotenv"), Format.Dotenv);
  assert.equal(formatByName("json5"), Format.Json5);
  assert.equal(formatByName("no-such-format"), null);
});

test("reads through the twin agree with the compiled format on every fixture", () => {
  for (const f of fixtures) {
    assert.deepEqual(parse(f.source, jsDotenv), parse(f.source, Format.Dotenv), f.name);
    assert.equal(convert(f.source, jsDotenv, Format.Json), convert(f.source, Format.Dotenv, Format.Json), f.name);
  }
});

test("the twin's printer writes what the compiled printer writes", () => {
  for (const f of fixtures) {
    const theirs = convert(f.source, Format.Dotenv, Format.Dotenv);
    // The twin printing its own tree, and printing the compiled parser's.
    assert.equal(convert(f.source, jsDotenv, jsDotenv), theirs, `${f.name}: own tree`);
    assert.equal(convert(f.source, Format.Dotenv, jsDotenv), theirs, `${f.name}: compiled tree`);
    // And a cross-format print into it: the same file a JSON document
    // becomes under either name.
    const json = convert(f.source, Format.Dotenv, Format.Json);
    assert.equal(convert(json, Format.Json, jsDotenv), convert(json, Format.Json, Format.Dotenv), `${f.name}: from JSON`);
  }
});

test("edits through the twin leave the same file the compiled format leaves", () => {
  const source = fixtures.find((f) => f.name === "secrets.env")!.source;
  const script = (ed: Editor) => {
    ed.set(["API_KEY"], "rotated");
    ed.set(["NEW_KEY"], "two words");
    ed.insertValue([], "AFTER", "x");
    ed.delete(["RAW"]);
    ed.addLeadingComment(["EMPTY"], "was empty");
    ed.setTrailingComment(["NEW_KEY"], "added");
    ed.deleteTrailingComment(["DB_URL"]);
    ed.replaceKey(["DB_URL"], "DATABASE_URL");
    return ed.source();
  };
  using mine = Editor.open(source, jsDotenv);
  using theirs = Editor.open(source, Format.Dotenv);
  const edited = script(mine);
  assert.equal(edited, script(theirs));
  // What the edit produced reads the same under both.
  assert.deepEqual(parse(edited, jsDotenv), parse(edited, Format.Dotenv));
  assert.equal(parse<Record<string, string>>(edited, jsDotenv).DATABASE_URL, "postgres://localhost/app");
});

test("editing an empty document seeds it from the dialect's empty_doc_seed", () => {
  using ed = Editor.open("", jsDotenv);
  ed.set(["A"], "1");
  assert.equal(ed.source(), "A=1\n");
});

test("a refusal by the language surfaces as a FigError with its message and offset", () => {
  assert.throws(
    () => Document.parse("KEY", jsDotenv),
    (err: unknown) => {
      assert.ok(err instanceof FigError);
      assert.equal(err.status, Status.ParseError);
      assert.match(err.message, /expected `=` after this key/);
      return true;
    },
  );
  assert.throws(
    () => Document.parse("A=1\n=2\n", jsDotenv),
    (err: unknown) => err instanceof FigError && /expected `KEY=value`/.test(err.message),
  );
  // The refusal the compiled parser makes too, in its words, at the offset
  // the language names: the opening quote. (The compiled format's own error
  // reaches this binding as a bare error name and no offset; a language
  // says more.)
  const caught = (format: Format) => {
    try {
      Document.parse("A=\"open\n", format);
    } catch (e) {
      return e as FigError;
    }
    return null;
  };
  const mine = caught(jsDotenv);
  assert.ok(caught(Format.Dotenv));
  assert.ok(mine);
  assert.match(mine.message, /unclosed quoted value/);
  assert.equal(mine.byteOffset, 2);
});

test("a language whose description fails validation registers nothing", () => {
  const bad = (patch: Partial<Language>, why: RegExp) => {
    assert.throws(
      () => registerLanguage({ ...dotenv, ...patch }),
      (err: unknown) => {
        assert.ok(err instanceof FigError, String(err));
        assert.equal(err.status, Status.InvalidArgument);
        assert.match(err.message, why);
        return true;
      },
    );
  };
  // A name the core already has, compiled or registered.
  bad({ name: "dotenv", dialects: [{ ...dotenv.dialects[0]!, name: "dotenv" }] }, /dotenv/);
  bad({}, /js-dotenv/);
  // A sample the language cannot parse: the harness runs at registration.
  bad({ name: "js-dotenv-broken", dialects: [{ ...dotenv.dialects[0]!, name: "js-dotenv-broken" }], samples: ["not a pair\n"] }, /sample/);
  // No samples at all.
  bad({ name: "js-dotenv-sampleless", dialects: [{ ...dotenv.dialects[0]!, name: "js-dotenv-sampleless" }], samples: [] }, /sample/);
  assert.equal(formatByName("js-dotenv-broken"), null);
  assert.equal(formatByName("js-dotenv-sampleless"), null);
});

test("a language's parse may throw anything; the core sees a refusal, not a crash", () => {
  const throwing: Language = {
    ...dotenv,
    name: "js-throws",
    dialects: [{ name: "js-throws", splice: "raw", empty_doc_seed: "" }],
    parse: (dialect, input) => {
      if (input.includes("boom")) throw new TypeError("boom");
      return dotenv.parse(dialect, input);
    },
  };
  const f = registerLanguage(throwing);
  assert.deepEqual(parse("A=1\n", f), { A: "1" });
  assert.throws(() => Document.parse("A=boom\n", f), (err: unknown) => err instanceof FigError && /boom/.test(err.message));
});

test("handle: the wire, one line to one line", () => {
  const d = JSON.parse(handle(dotenv, '{"op":"describe"}')) as { ok: boolean; description: Record<string, unknown> };
  assert.equal(d.ok, true);
  assert.deepEqual(d.description, describe(dotenv));
  assert.equal(d.description.name, "js-dotenv");
  assert.deepEqual(d.description.renderers, []);
  assert.deepEqual((d.description.dialects as unknown[])[0], { name: "js-dotenv", extensions: ["env"], splice: "raw", empty_doc_seed: "", syntax: null });

  const refused = JSON.parse(handle(dotenv, JSON.stringify({ op: "parse", dialect: "js-dotenv", input: "A" }))) as Record<string, unknown>;
  assert.equal(refused.ok, false);
  assert.match(refused.message as string, /expected `=`/);
  assert.equal(refused.byte_offset, 0);

  const printed = JSON.parse(
    handle(dotenv, JSON.stringify({ op: "print", dialect: "js-dotenv", table: parsedThroughWire(dotenv, "A=1 # c\n"), options: { pretty: true } })),
  ) as Record<string, unknown>;
  assert.deepEqual(printed, { ok: true, output: "A=1 # c\n" });

  assert.deepEqual(JSON.parse(handle(dotenv, '{"op":"nope"}')), { ok: false, message: 'unknown op "nope"' });
  assert.deepEqual(JSON.parse(handle(dotenv, "not json")).ok, false);
  assert.deepEqual(JSON.parse(handle(dotenv, JSON.stringify({ op: "render", which: "value", dialect: "js-dotenv", value: "x" }))).ok, false);
});

test("serve: the same wire over any line source", async () => {
  async function* lines() {
    yield '{"op":"describe"}\n';
    yield "\n";
    yield '{"op":"parse","dialect":"js-dot'; // a request split across chunks
    yield 'env","input":"A=1\\n"}\n{"op":"nope"}';
  }
  const out: string[] = [];
  await serve(dotenv, { input: lines(), output: (line) => void out.push(line) });
  assert.equal(out.length, 3);
  assert.equal((JSON.parse(out[0]!) as { description: { name: string } }).description.name, "js-dotenv");
  assert.deepEqual((JSON.parse(out[1]!) as { table: NodeTable }).table.rows.length, 4);
  assert.equal((JSON.parse(out[2]!) as { ok: boolean }).ok, false);
});

test("the helper subpath loads the wire and nothing the wasm module is needed for", () => {
  // `@diaryx/fig/helper` is what a helper process and an embedding runner
  // import; it must never pull the module bytes in. Read as source: an
  // import that reaches `ffi.ts` or `wasm-bytes.ts`, directly or through
  // `language.ts`, is the regression.
  for (const file of ["wire.ts", "helper.ts"]) {
    const source = readFileSync(join(process.cwd(), "src", file), "utf8");
    const imports = [...source.matchAll(/from "\.\/([^"]+)"/g)].map((m) => m[1]!);
    assert.ok(imports.length > 0 || file === "wire.ts", `${file} imports something`);
    for (const target of imports) {
      assert.ok(!/^(ffi|wasm-bytes|language|types)\.ts$/.test(target), `${file} imports ${target}`);
    }
  }
  const pkg = JSON.parse(readFileSync(join(process.cwd(), "package.json"), "utf8")) as { exports: Record<string, { default: string }> };
  assert.equal(pkg.exports["./helper"]?.default, "./dist/helper.js");
});

test("a runtime printer is told when it prints splice text", () => {
  const seen: boolean[] = [];
  const spy = registerLanguage({
    ...dotenv,
    name: "js-dotenv-splice",
    dialects: [{ ...dotenv.dialects[0]!, name: "js-dotenv-splice", extensions: ["env-splice"] }],
    print: (d, t, o) => {
      seen.push(o.splice);
      return dotenv.print!(d, t, o);
    },
  });
  seen.length = 0; // registration prints the samples, as a document
  using ed = Editor.open("A=1\n", spy);
  ed.insertValue([], "B", "two");
  assert.equal(ed.source(), "A=1\nB=two\n");
  assert.deepEqual(seen, [true]);
  seen.length = 0;
  serialize({ C: "3" }, spy);
  assert.deepEqual(seen, [false]);
});
