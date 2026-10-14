// The formats in `js/` (`@diaryx/fig/js/*`), each held to
// the Zig language of the same name. The contract is the format and the
// tree, not the compiled parser's code or its messages:
//
//   * **table for table** — `test/fixtures/<dialect>/*` are documents, and
//     beside each, `*.table.json` is the node table the compiled format
//     gives for it, printed by `fig lang table -i <dialect>`; regenerate
//     one with that command when the compiled format changes. The
//     language's `parse` must give the same table: every row, span, text
//     and comment;
//   * **through fig** — registered, a language prints every fixture to
//     the bytes the compiled printer wrote (`*.printed`), and converts it
//     to the JSON the compiled format converted it to (`*.converted`), both
//     recorded by `scripts/record-compiled.mjs` from a module that
//     compiles every format in.
//
// The module these tests load compiles no format in, as the published one
// does not, so each language registers under its own name, and each of its
// dialects is a format of its own, found by name.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

import { capabilities, convert, detectFormat, Editor, formatByName, formatName, handle, splitStream, registerLanguage, type Language, type NodeTable } from "../src/index.ts";

import canonical from "@diaryx/fig/js/canonical";
import dotenv from "@diaryx/fig/js/dotenv";
import fig from "@diaryx/fig/js/figl";
import ini from "@diaryx/fig/js/ini";
import json from "@diaryx/fig/js/json";
import json5 from "@diaryx/fig/js/json5";
import nestedtext from "@diaryx/fig/js/nestedtext";
import plist from "@diaryx/fig/js/plist";
import properties from "@diaryx/fig/js/properties";
import toml from "@diaryx/fig/js/toml";
import yaml from "@diaryx/fig/js/yaml";
import zon from "@diaryx/fig/js/zon";

/** A language, and its dialects' fixture directories and file extensions. */
interface Twin {
  lang: Language;
  dialects: { name: string; ext: string }[];
}

const TWINS: Twin[] = [
  { lang: json, dialects: [{ name: "json", ext: "json" }] },
  { lang: json5, dialects: [{ name: "json5", ext: "json5" }, { name: "jsonc", ext: "jsonc" }] },
  { lang: yaml, dialects: [{ name: "yaml", ext: "yaml" }, { name: "yaml-1.1", ext: "yaml" }] },
  { lang: toml, dialects: [{ name: "toml", ext: "toml" }] },
  { lang: ini, dialects: [{ name: "ini", ext: "ini" }] },
  { lang: fig, dialects: [{ name: "figl", ext: "figl" }] },
  { lang: dotenv, dialects: [{ name: "dotenv", ext: "env" }] },
  { lang: properties, dialects: [{ name: "properties", ext: "properties" }] },
  { lang: nestedtext, dialects: [{ name: "nestedtext", ext: "nt" }] },
  { lang: zon, dialects: [{ name: "zon", ext: "zon" }] },
  { lang: plist, dialects: [{ name: "plist", ext: "plist" }] },
  // No format of fig's registry: registered under its own name, it is a
  // new format, and held to its tables alone.
  { lang: canonical, dialects: [{ name: "canonical", ext: "canonical" }] },
];

interface Fixture {
  name: string;
  source: string;
  table: NodeTable;
  /** What the compiled printer wrote for it, and what the compiled format
   *  converted it to JSON as, where those were recorded. */
  printed: string | null;
  converted: string | null;
}

// `npm test` runs from the package directory.
function fixtures(dialect: string, ext: string): Fixture[] {
  const dir = join(process.cwd(), "test", "fixtures", dialect);
  return readdirSync(dir)
    .filter((f) => f.endsWith(`.${ext}`) && !f.endsWith(".table.json"))
    .sort()
    .map((f) => {
      const stem = f.slice(0, -(ext.length + 1));
      const recorded = (suffix: string) => {
        const path = join(dir, `${stem}.${suffix}`);
        return existsSync(path) ? readFileSync(path, "utf8") : null;
      };
      return {
        name: `${dialect}/${f}`,
        source: readFileSync(join(dir, f), "utf8"),
        table: JSON.parse(readFileSync(join(dir, `${stem}.table.json`), "utf8")) as NodeTable,
        printed: recorded("printed"),
        converted: recorded("converted"),
      };
    });
}

/** `v` with every null or absent field dropped, so a table the language
 *  writes and one the compiled format wrote compare equal whichever side
 *  spelled an empty column. */
function canonicalTable(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(canonicalTable);
  if (v !== null && typeof v === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) if (x !== null && x !== undefined) out[k] = canonicalTable(x);
    return out;
  }
  return v;
}

function parsedThroughWire(lang: Language, dialect: string, input: string): NodeTable {
  const resp = JSON.parse(handle(lang, JSON.stringify({ jsonrpc: "2.0", id: 1, method: "parse", params: { dialect, input } }))) as { result?: NodeTable; error?: { message: string } };
  assert.ok(resp.result, resp.error?.message);
  return resp.result!;
}

test("each language gives the compiled format's table for every fixture, row for row", () => {
  for (const t of TWINS) {
    for (const d of t.dialects) {
      const fs = fixtures(d.name, d.ext);
      assert.ok(fs.length > 0, `${d.name}: fixtures present`);
      for (const f of fs) {
        assert.deepEqual(canonicalTable(parsedThroughWire(t.lang, d.name, f.source)), canonicalTable(f.table), f.name);
      }
    }
  }
});

// Registered once each; a format is per process and never unregistered.
const registered = TWINS.map((t) => ({ ...t, format: registerLanguage(t.lang) }));

/** `source` read as `format` and written by `write`, or `error: ` and the
 *  message it was refused with — as `scripts/record-compiled.mjs` records
 *  the compiled format's answer. */
function attempt(write: () => string): string {
  try {
    return write();
  } catch (e) {
    return `error: ${(e as Error).message}\n`;
  }
}

test("each language's format is its first dialect's, and every dialect is a format found by name", () => {
  const seen = new Set<number>();
  for (const t of registered) {
    assert.equal(t.lang.format, t.format, `${t.lang.name}.format`);
    assert.equal(formatName(t.format), t.lang.name);
    for (const d of t.dialects) {
      const format = formatByName(d.name);
      assert.ok(format !== null, d.name);
      assert.equal(formatName(format), d.name);
      assert.ok(!seen.has(format), `${d.name} has a handle of its own`);
      seen.add(format);
    }
  }
  // Handles are assigned in registration order from 1; nothing was
  // registered in this process before JSON.
  assert.equal(json.format, 1);
  // JSON5's second dialect is a format beside its first.
  assert.notEqual(formatByName("jsonc"), json5.format);
  assert.equal(capabilities(yaml.format!).references, true);
  // A handle never handed out names nothing.
  assert.equal(formatName(0 as never), null);
  assert.equal(formatName(100000 as never), null);
  assert.equal(formatByName("no-such-format"), null);
  // A spread of a registered language is a new object, carrying no format.
  assert.equal({ ...yaml }.format, undefined);
});

test("detectFormat tries the registered languages in the caller's order", () => {
  assert.equal(detectFormat('{"a": 1}', ["json", "yaml"]), json.format!);
  assert.equal(detectFormat('{"a": 1}', ["yaml", "json"]), yaml.format!);
  assert.equal(detectFormat("a = 1\n", ["nosuch", "json", "toml"]), toml.format!);
  assert.equal(detectFormat('{"a": ', ["json"]), null);
});

test("splitStream locates a YAML stream's documents through the twin", () => {
  const src = "# lead\n%YAML 1.2\n---\na: 1\n...\nb: 2\n--- é\n";
  const docs = splitStream(src, yaml.format!);
  assert.deepEqual(
    docs.map((d) => [d.text, d.explicit]),
    [
      ["# lead\n%YAML 1.2\n---\na: 1\n", true],
      ["b: 2\n", false],
      ["--- é\n", true],
    ],
  );
  assert.deepEqual(splitStream("", yaml.format!), [{ text: "", start: 0, end: 0, explicit: false }]);
  assert.throws(() => splitStream("{}", json.format!));
});

test("the YAML twin refuses a line indented under an entry whose node is whole, as the compiled format does", () => {
  // A plain value takes such a line as its continuation
  // (fixtures/yaml/plain-continuation.yaml); any other value cannot, and
  // a `: ` is not plain text. PyYAML refuses each of these.
  for (const src of [
    "- a\n  : b\n", "- a\n  b: c\n", "- a\n  - b: c\n", "- a\n  # c\n  - b\n", "- a # c\n  - b\n",
    "- \"a\"\n  - b\n", "- 'a'\n  - b\n", "? a\n  : b\n", "? a\n  b: c\n", "- k: a\n  b\n",
  ]) {
    assert.throws(() => convert(src, yaml.format!, json.format!), Error, JSON.stringify(src));
  }
});

test("each language prints every fixture as the compiled printer did", () => {
  for (const t of TWINS) {
    for (const d of t.dialects) {
      const format = formatByName(d.name)!;
      for (const f of fixtures(d.name, d.ext)) {
        if (f.printed === null) continue;
        assert.equal(attempt(() => convert(f.source, format, format)), f.printed, f.name);
      }
    }
  }
});

test("each language converts every fixture to the JSON the compiled format did", () => {
  const recorded = TWINS.flatMap((t) => t.dialects.flatMap((d) => fixtures(d.name, d.ext).filter((f) => f.converted !== null)));
  assert.ok(recorded.length > 100, "recordings present");
  for (const t of TWINS) {
    for (const d of t.dialects) {
      const format = formatByName(d.name)!;
      for (const f of fixtures(d.name, d.ext)) {
        if (f.converted === null) continue;
        assert.equal(attempt(() => convert(f.source, format, json.format!)), f.converted, f.name);
      }
    }
  }
});

test("figl writes a non-finite float from another format under : float, as the compiled printer does", () => {
  // Bare, figl reads `inf` as a string; the compiled printer wrote this.
  const figl = formatByName("figl")!;
  assert.equal(convert("a: .inf\nb: [1, -.inf]\nc: .nan\n", yaml.format!, figl), "a: float = inf\n\nb\n> * 1\n> *: float = -inf\n\nc: float = nan\n");
});

test("the figl twin's comment on a multi-line flow list or object is on its opener line, as figl's is", () => {
  // The parser reads `xs = [ # op` as the list's trailing comment, so the
  // editor's trailing-comment ops act there, not after the close.
  const figl = formatByName("figl")!;
  const shapes: [string, string, string][] = [["[", "  1,\n  2,\n", "]"], ["{", "  a = 1,\n  b = 2,\n", "}"]];
  for (const [open, body, close] of shapes) {
    for (const nl of ["\n", "\r\n"]) {
      const b = body.replace(/\n/g, nl);
      using ed = Editor.open(`xs = ${open} # op${nl}${b}${close}${nl}y = 1${nl}`, figl);
      assert.equal(ed.getTrailingComment(["xs"]), "op");
      ed.setTrailingComment(["xs"], "note");
      assert.equal(ed.source(), `xs = ${open} # note${nl}${b}${close}${nl}y = 1${nl}`);
      ed.deleteTrailingComment(["xs"]);
      assert.equal(ed.source(), `xs = ${open}${nl}${b}${close}${nl}y = 1${nl}`);
      assert.equal(ed.getTrailingComment(["xs"]), null);
    }
  }
  // A flow list on one line keeps its comment after the close.
  using one = Editor.open("xs = [1, 2]\n", figl);
  one.setTrailingComment(["xs"], "note");
  assert.equal(one.source(), "xs = [1, 2] # note\n");
});
