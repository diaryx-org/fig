// Record what the compiled formats make of every fixture in test/fixtures,
// for test/twins.test.ts to hold the languages to: beside each
// `<name>.<ext>`, `<name>.printed` is the document printed back in its own
// format and `<name>.converted` the document converted to JSON (or `error: ` and
// the message, where the conversion is refused). The tables beside them
// (`<name>.table.json`) come from `fig lang table -i <format>`.
//
// The published module compiles no format in, so this runs against a
// reference module that compiles them all:
//
//   FIG_WASM_LANGUAGES=all npm run build:wasm
//   node scripts/record-compiled.mjs
//   npm run build:wasm
//
// Rerun it when a compiled format's printer changes, and read the diff: a
// recording that moves is a change the languages must follow.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import { Format, capabilities, convert } from "../src/index.ts";

const fixtures = join(dirname(fileURLToPath(import.meta.url)), "..", "test", "fixtures");

// Each fixture directory the module can select a compiled format for, and
// its files' extension. yaml-1.1 has no selection in this binding, and the
// canonical form is not a module option; both are held to their tables alone.
const DIRS = [
  ["json", Format.Json, "json"],
  ["jsonc", Format.Jsonc, "jsonc"],
  ["json5", Format.Json5, "json5"],
  ["yaml", Format.Yaml, "yaml"],
  ["toml", Format.Toml, "toml"],
  ["zon", Format.Zon, "zon"],
  ["fig", Format.Fig, "figl"],
  ["ini", Format.Ini, "ini"],
  ["dotenv", Format.Dotenv, "env"],
  ["properties", Format.Properties, "properties"],
  ["plist", Format.Plist, "plist"],
  ["nestedtext", Format.Nestedtext, "nt"],
];

for (const [, format] of DIRS) {
  if (!capabilities(format).read) {
    console.error("record-compiled: this module does not compile every format in; build it with FIG_WASM_LANGUAGES=all first");
    process.exit(1);
  }
}

const attempt = (f) => {
  try {
    return f();
  } catch (e) {
    return `error: ${e.message}\n`;
  }
};

let n = 0;
for (const [dir, format, ext] of DIRS) {
  for (const file of readdirSync(join(fixtures, dir)).sort()) {
    if (!file.endsWith(`.${ext}`) || file.endsWith(".table.json")) continue;
    const stem = join(fixtures, dir, file.slice(0, -(ext.length + 1)));
    const source = readFileSync(join(fixtures, dir, file), "utf8");
    writeFileSync(`${stem}.printed`, attempt(() => convert(source, format, format)));
    writeFileSync(`${stem}.converted`, attempt(() => convert(source, format, Format.Json)));
    n++;
  }
}
console.error(`record-compiled: recorded ${n} fixtures`);
