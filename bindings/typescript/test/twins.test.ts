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
//   * **through fig** — registered, a language reads every fixture to the
//     value the compiled format reads, and prints it to the bytes the
//     compiled printer writes.
//
// The module these tests load compiles in the default formats and leaves
// ZON and plist out. A language whose format is compiled in cannot take
// its name, so it registers under a prefix, beside the format it is
// compared with; ZON and plist register under their own names and stand in
// for their formats, which is what every language does in a module that
// compiles none in.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

import { Format, capabilities, convert, formatByName, handle, parse, registerLanguage, type Language, type NodeTable } from "../src/index.ts";

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

/** A language, its dialects' fixture directories and file extensions, and
 *  whether the module these tests load compiles its format in. */
interface Twin {
  lang: Language;
  dialects: { name: string; ext: string }[];
  compiled: boolean;
}

const TWINS: Twin[] = [
  { lang: json, dialects: [{ name: "json", ext: "json" }], compiled: true },
  { lang: json5, dialects: [{ name: "json5", ext: "json5" }, { name: "jsonc", ext: "jsonc" }], compiled: true },
  { lang: yaml, dialects: [{ name: "yaml", ext: "yaml" }, { name: "yaml-1.1", ext: "yaml" }], compiled: true },
  { lang: toml, dialects: [{ name: "toml", ext: "toml" }], compiled: true },
  { lang: ini, dialects: [{ name: "ini", ext: "ini" }], compiled: true },
  { lang: fig, dialects: [{ name: "fig", ext: "figl" }], compiled: true },
  { lang: dotenv, dialects: [{ name: "dotenv", ext: "env" }], compiled: true },
  { lang: properties, dialects: [{ name: "properties", ext: "properties" }], compiled: true },
  { lang: nestedtext, dialects: [{ name: "nestedtext", ext: "nt" }], compiled: true },
  { lang: zon, dialects: [{ name: "zon", ext: "zon" }], compiled: false },
  { lang: plist, dialects: [{ name: "plist", ext: "plist" }], compiled: false },
  // No format of fig's registry: registered under its own name, it is a
  // new format beside the compiled-out canonical form.
  { lang: canonical, dialects: [{ name: "canonical", ext: "canonical" }], compiled: false },
];

interface Fixture {
  name: string;
  source: string;
  table: NodeTable;
  /** What the compiled printer wrote for it, where that was recorded. */
  printed: string | null;
}

// `npm test` runs from the package directory.
function fixtures(dialect: string, ext: string): Fixture[] {
  const dir = join(process.cwd(), "test", "fixtures", dialect);
  return readdirSync(dir)
    .filter((f) => f.endsWith(`.${ext}`) && !f.endsWith(".table.json"))
    .sort()
    .map((f) => {
      const stem = f.slice(0, -(ext.length + 1));
      const printed = join(dir, `${stem}.printed`);
      return {
        name: `${dialect}/${f}`,
        source: readFileSync(join(dir, f), "utf8"),
        table: JSON.parse(readFileSync(join(dir, `${stem}.table.json`), "utf8")) as NodeTable,
        printed: existsSync(printed) ? readFileSync(printed, "utf8") : null,
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

/** `lang` under names `prefix` + its own, told its own names back: every
 *  dialect the core hands it has the prefix taken off first, so a language
 *  that tells its dialects apart by name keeps doing so. */
function prefixed(lang: Language, prefix: string): Language {
  const bare = (dialect: string) => (dialect.startsWith(prefix) ? dialect.slice(prefix.length) : dialect);
  const out: Language = {
    ...lang,
    name: prefix + lang.name,
    dialects: lang.dialects.map((d) => ({ ...d, name: prefix + d.name })),
    parse: (dialect, input) => lang.parse(bare(dialect), input),
  };
  if (lang.print) {
    const print = lang.print.bind(lang);
    out.print = (dialect, table, options) => print(bare(dialect), table, options);
  }
  if (lang.render) {
    const render = lang.render.bind(lang);
    out.render = (which, args) => render(which, { ...args, dialect: bare(args.dialect) });
  }
  return out;
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
const PREFIX = "twin-";
const registered = TWINS.map((t) => ({ ...t, format: registerLanguage(t.compiled ? prefixed(t.lang, PREFIX) : t.lang) }));

/** The format a dialect of `t` was registered as. */
function formatOf(t: (typeof registered)[number], dialect: string): Format {
  const f = formatByName((t.compiled ? PREFIX : "") + dialect);
  assert.notEqual(f, null, dialect);
  return f!;
}

test("a language whose format the module leaves out stands in for it, at its own number", () => {
  const byName = (name: string) => registered.find((t) => t.lang.name === name)!;
  assert.equal(byName("zon").format, Format.Zon);
  assert.equal(byName("plist").format, Format.Plist);
  assert.equal(formatByName("zon"), Format.Zon);
  assert.equal(capabilities(Format.Zon).edit, true);
  // A language compiled in keeps the name; its twin is beside it.
  assert.equal(formatByName("yaml"), Format.Yaml);
  assert.ok(byName("yaml").format >= 4096);
});

/** `source` read as `format` into plain values, or the message reading it
 *  failed with: a document with a YAML alias is not read into values by
 *  either side yet (docs/tasks/values-through-an-alias.md), and the two
 *  should fail alike. */
function read(source: string, format: Format): unknown {
  try {
    return parse(source, format);
  } catch (e) {
    return (e as Error).message;
  }
}

/** `source` converted from `format` to JSON, or the message the
 *  conversion failed with: a value JSON cannot hold refuses the conversion,
 *  and should refuse it alike. */
function toJson(source: string, format: Format): string {
  try {
    return convert(source, format, Format.Json);
  } catch (e) {
    return (e as Error).message;
  }
}

test("each language reads every fixture to the value the compiled format reads", () => {
  for (const t of registered.filter((t) => t.compiled)) {
    for (const d of t.dialects) {
      const mine = formatOf(t, d.name);
      // The package selects no YAML 1.1 of the compiled format — only the
      // table above holds that dialect to it.
      const theirs = formatByName(d.name);
      if (theirs === null) continue;
      for (const f of fixtures(d.name, d.ext)) {
        assert.deepEqual(read(f.source, mine), read(f.source, theirs), f.name);
        assert.equal(toJson(f.source, mine), toJson(f.source, theirs), f.name);
      }
    }
  }
});

test("each language prints every fixture as the compiled printer does", () => {
  for (const t of registered.filter((t) => t.compiled)) {
    for (const d of t.dialects) {
      const mine = formatOf(t, d.name);
      const theirs = formatByName(d.name);
      if (theirs === null) continue;
      for (const f of fixtures(d.name, d.ext)) {
        const want = convert(f.source, theirs, theirs);
        // The language printing its own tree, and printing the compiled
        // parser's.
        assert.equal(convert(f.source, mine, mine), want, `${f.name}: own tree`);
        assert.equal(convert(f.source, theirs, mine), want, `${f.name}: compiled tree`);
      }
    }
  }
});

test("a stand-in prints each fixture as the compiled printer did", () => {
  for (const f of fixtures("zon", "zon")) {
    if (f.printed === null) continue;
    assert.equal(convert(f.source, Format.Zon, Format.Zon), f.printed, f.name);
  }
});
