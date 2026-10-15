// Differential parity: run the same `fig` command lines through two builds
// of the CLI (the Zig reference and the Rust port) and score how often they
// agree.
//
//   node rust/parity/cli-diff.mjs --reference <zig fig> --candidate <rust fig>
//        [--corpus <dir>]… [--max-per-dir N] [--failures out.json] [--only group]
//
// For every document in the corpora it runs a matrix of command lines in a
// scratch directory of its own, one copy of the document per command:
//
//   read     get, get -o json/yaml/gron, a scalar path, check, fmt --dry-run
//   convert  convert -o <each format the CLI writes>
//   edit     replace, set, insert, delete, rename, comment, comment --inline,
//            on paths taken from the reference's own `get -o gron`
//
// A command agrees when the exit status, stdout and (for edits) the edited
// file are byte-identical. stderr is compared too, but reported apart:
// messages may be reworded without breaking anyone's script.
//
// The corpora default to fig-conformance's edit sources and a sample of its
// parse corpora (a checkout beside this one, or $FIG_CONFORMANCE).
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const here = dirname(fileURLToPath(import.meta.url));
const { values: opts } = parseArgs({
  options: {
    reference: { type: "string" },
    candidate: { type: "string" },
    corpus: { type: "string", multiple: true },
    "max-per-dir": { type: "string", default: "40" },
    failures: { type: "string" },
    only: { type: "string" },
    json: { type: "boolean" },
  },
});
if (!opts.reference || !opts.candidate) {
  console.error("cli-diff: --reference <fig> and --candidate <fig> are required");
  process.exit(2);
}
const reference = resolve(opts.reference);
const candidate = resolve(opts.candidate);
const maxPerDir = Number(opts["max-per-dir"]);

const EXT = {
  json: "json", jsonc: "jsonc", json5: "json5", yaml: "yaml", yml: "yaml", toml: "toml", zon: "zon",
  figl: "figl", ini: "ini", env: "dotenv", properties: "properties", plist: "plist", nt: "nestedtext",
};
const WRITES = ["json", "jsonc", "json5", "yaml", "toml", "zon", "figl", "ini", "dotenv", "properties", "plist", "nestedtext"];

function corpusDirs() {
  if (opts.corpus?.length) return opts.corpus.map((d) => resolve(d));
  const conf = resolve(process.env.FIG_CONFORMANCE ?? join(here, "..", "..", "..", "diaryx-org", "fig-conformance"));
  const alt = resolve(join(here, "..", "..", "..", "fig-conformance"));
  const root = existsSync(conf) ? conf : alt;
  if (!existsSync(root)) {
    console.error("cli-diff: no fig-conformance checkout; pass --corpus <dir> or set FIG_CONFORMANCE");
    process.exit(2);
  }
  const dirs = [];
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
    }
    // An edit suite's own directory holds its case files, not documents;
    // its documents are in `sources/`.
    if (basename(d) === "edits") return;
    if (readdirSync(d).some((f) => EXT[extname(f).slice(1)])) dirs.push(d);
  };
  walk(join(root, "suites"));
  return dirs;
}

function documents() {
  const out = [];
  for (const dir of corpusDirs()) {
    const files = readdirSync(dir)
      .filter((f) => EXT[extname(f).slice(1)] && statSync(join(dir, f)).isFile() && statSync(join(dir, f)).size < 64 * 1024)
      .sort();
    // An even sample, so a big corpus is represented across its range.
    const step = Math.max(1, Math.ceil(files.length / maxPerDir));
    for (let i = 0; i < files.length; i += step) out.push(join(dir, files[i]));
  }
  return out;
}

function run(bin, args, cwd, input) {
  const r = spawnSync(bin, args, { cwd, input, encoding: "buffer", timeout: 20000, env: { ...process.env, NO_COLOR: "1" } });
  return { status: r.status ?? (r.signal ? `signal ${r.signal}` : "?"), stdout: r.stdout ?? Buffer.alloc(0), stderr: r.stderr ?? Buffer.alloc(0) };
}

/** The gron paths of `file` per the reference: `json.a.b[0] = 1;` → `a.b[0]`. */
function gronPaths(file) {
  const dir = mkdtempSync(join(tmpdir(), "fig-diff-"));
  try {
    const f = join(dir, basename(file));
    copyFileSync(file, f);
    const r = run(reference, ["get", basename(file), "-o", "gron"], dir);
    if (r.status !== 0) return [];
    return r.stdout.toString().split("\n")
      .map((l) => l.match(/^json((?:\.|\[).*?) = (.*);$/))
      .filter(Boolean)
      .map(([, p, v]) => ({ path: p.replace(/^\./, ""), container: v === "{}" || v === "[]" }));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function matrix(file) {
  const name = basename(file);
  const cmds = [];
  const add = (group, args, edits = false) => cmds.push({ group, args, edits });
  add("read", ["get", name]);
  add("read", ["get", name, "-o", "json"]);
  add("read", ["get", name, "-o", "yaml"]);
  add("read", ["get", name, "-o", "gron"]);
  add("read", ["check", name]);
  add("read", ["fmt", "--dry-run", name]);
  for (const to of WRITES) add("convert", ["convert", "-o", to, name]);
  const paths = gronPaths(file);
  const scalars = paths.filter((p) => !p.container && p.path);
  const containers = paths.filter((p) => p.container && p.path);
  const pick = (list, n) => {
    const step = Math.max(1, Math.floor(list.length / n));
    return list.filter((_, i) => i % step === 0).slice(0, n).map((p) => p.path);
  };
  for (const p of pick(scalars, 3)) {
    add("read", ["get", name, p]);
    add("edit", ["replace", name, p, "42"], true);
    add("edit", ["replace", name, p, "hello world"], true);
    add("edit", ["delete", name, p], true);
    add("edit", ["comment", name, p, "a note"], true);
    add("edit", ["comment", "--inline", name, p, "a note"], true);
    if (!p.endsWith("]")) add("edit", ["rename", name, p, "renamed"], true);
  }
  for (const p of pick(containers, 2)) {
    add("edit", ["set", name, `${p}.added`, "1"], true);
    add("edit", ["delete", name, p], true);
  }
  add("edit", ["set", name, "fresh_key", "true"], true);
  add("edit", ["insert", name, "new_key", "[1, 2]"], true);
  return cmds;
}

function attempt(bin, file, cmd) {
  const dir = mkdtempSync(join(tmpdir(), "fig-diff-"));
  try {
    copyFileSync(file, join(dir, basename(file)));
    const r = run(bin, cmd.args, dir);
    const after = cmd.edits && existsSync(join(dir, basename(file))) ? readFileSync(join(dir, basename(file))) : null;
    return { ...r, after };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const docs = documents();
const groups = {};
const failures = [];
let stderrOnly = 0;
for (const file of docs) {
  for (const cmd of matrix(file)) {
    if (opts.only && cmd.group !== opts.only) continue;
    const a = attempt(reference, file, cmd);
    const b = attempt(candidate, file, cmd);
    const g = (groups[cmd.group] ??= { agree: 0, total: 0 });
    g.total++;
    const same = String(a.status) === String(b.status) && a.stdout.equals(b.stdout) &&
      (a.after === null ? b.after === null : b.after !== null && a.after.equals(b.after));
    if (same) {
      g.agree++;
      if (!a.stderr.equals(b.stderr)) stderrOnly++;
    } else {
      failures.push({
        file, args: cmd.args,
        reference: { status: a.status, stdout: a.stdout.toString(), stderr: a.stderr.toString(), after: a.after?.toString() },
        candidate: { status: b.status, stdout: b.stdout.toString(), stderr: b.stderr.toString(), after: b.after?.toString() },
      });
    }
  }
}
let agree = 0, total = 0;
for (const g of Object.values(groups)) { agree += g.agree; total += g.total; }
const pct = (a, t) => (t ? ((100 * a) / t).toFixed(1) : "-");
if (opts.json) {
  console.log(JSON.stringify({ documents: docs.length, groups, agree, total, stderrOnly }, null, 2));
} else {
  console.log(`${docs.length} documents`);
  for (const [name, g] of Object.entries(groups)) console.log(`${name.padEnd(8)} ${g.agree}/${g.total} (${pct(g.agree, g.total)}%)`);
  console.log(`all      ${agree}/${total} (${pct(agree, total)}%), ${stderrOnly} agreeing only up to stderr`);
}
if (opts.failures) writeFileSync(opts.failures, JSON.stringify(failures, null, 1) + "\n");
