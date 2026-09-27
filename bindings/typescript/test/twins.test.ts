// The formats in `languages/` (`@diaryx/fig/languages/*`), each held to
// the compiled format it stands in for. The contract is the format and the
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
// does not, so each language registers under its own name and stands in
// for its format at that format's number.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

import { Format, capabilities, convert, formatByName, handle, registerLanguage, type Language, type NodeTable } from "../src/index.ts";

import canonical from "@diaryx/fig/languages/canonical";
import dotenv from "@diaryx/fig/languages/dotenv";
import fig from "@diaryx/fig/languages/fig";
import ini from "@diaryx/fig/languages/ini";
import json from "@diaryx/fig/languages/json";
import json5 from "@diaryx/fig/languages/json5";
import nestedtext from "@diaryx/fig/languages/nestedtext";
import plist from "@diaryx/fig/languages/plist";
import properties from "@diaryx/fig/languages/properties";
import toml from "@diaryx/fig/languages/toml";
import yaml from "@diaryx/fig/languages/yaml";
import zon from "@diaryx/fig/languages/zon";

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
  { lang: fig, dialects: [{ name: "fig", ext: "figl" }] },
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
  const resp = JSON.parse(handle(lang, JSON.stringify({ op: "parse", dialect, input }))) as { ok: boolean; table?: NodeTable; message?: string };
  assert.ok(resp.ok, resp.message);
  return resp.table!;
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

test("each language stands in for its format, at the format's own number", () => {
  const byName = (name: string) => registered.find((t) => t.lang.name === name)!;
  assert.equal(byName("json").format, Format.Json);
  assert.equal(byName("json5").format, Format.Json5);
  assert.equal(formatByName("jsonc"), Format.Jsonc);
  assert.equal(byName("yaml").format, Format.Yaml);
  assert.equal(byName("zon").format, Format.Zon);
  assert.equal(byName("plist").format, Format.Plist);
  assert.equal(capabilities(Format.Yaml).references, true);
  // A dialect no compiled format has a row for is a runtime number of its
  // own, found by name.
  assert.ok(formatByName("yaml-1.1")! >= 4096);
  assert.ok(byName("canonical").format >= 4096);
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
        assert.equal(attempt(() => convert(f.source, format, Format.Json)), f.converted, f.name);
      }
    }
  }
});
