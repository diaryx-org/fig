// Dev tool: run the edit fixtures, `languages/<name>/tests/edits.json`, against
// an implementation of fig's editor, and fail if any step comes out otherwise.
//
//   node --conditions=fig-source tools/edit-fixtures.mjs [--command <cmd>] [name…]
//
// A fixture pins one behaviour the hand-written editor tests pinned, in terms
// any implementation can be held to: a source, then edits applied in turn,
// each the output of the one before (a refused edit leaves the source as it
// was). Each edit is one of fig's `Editor` methods, with its arguments as
// JSON, and expects either the whole edited source or a refusal with a status.
// Statuses are compared; messages are not. An expectation with no `source`
// holds only that the edit is taken, and a refusal with no `status` only that
// it is refused: what the test asserted of that step, and no more. A fixture
// names the dialect its cases are read as, and a case may name another (a
// JSON5 case in the JSON fixture). Most ops are in fig-conformance's
// adapter protocol, under the same names; a few the protocol does not carry
// (`moveContainer`, `reorderContainers`, the `*Raw` edits that take text the
// format spells rather than a value) are the TypeScript binding's, and an
// adapter that serves these fixtures answers them too.
//
// With no `--command` the implementation is fig's own: the TypeScript binding
// in-process, with each language's WebAssembly module (`languages/<name>/js/`,
// built by `zig build wasm && node js/build.mjs` in the language's directory)
// registered, so the fixtures hold the Zig editor. `--command` runs any
// implementation that speaks the protocol's `hello` and `edit` methods on its
// stdin and stdout (fig-conformance's docs/adapters.md), such as another port
// of the editor.
//
// The fixtures are separate from fig-conformance's edit suites, which are
// generated from each source's tree and recorded from fig. These are written
// by hand, each for a reason its `note` gives, and their expectations are the
// ones the Zig tests asserted, not recorded.
import { spawn } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { values: opts, positionals: only } = parseArgs({
  options: { command: { type: "string" } },
  allowPositionals: true,
});

const names = readdirSync(join(root, "languages"))
  .filter((name) => existsSync(fixturePath(name)))
  .filter((name) => !only.length || only.includes(name))
  .sort();
for (const name of only) {
  if (!names.includes(name)) die(`edit-fixtures: languages/${name} has no tests/edits.json`);
}

const impl = opts.command ? await commandImpl(opts.command) : await figImpl(names);
let passed = 0, failed = 0;
for (const name of names) {
  const fixture = JSON.parse(readFileSync(fixturePath(name), "utf8"));
  let ok = 0;
  for (const c of fixture.cases) {
    const why = await runCase(c.dialect ?? fixture.dialect, c);
    if (why) {
      failed++;
      console.log(`FAIL ${name}: ${c.id}\n${why}`);
    } else {
      ok++;
      passed++;
    }
  }
  console.log(`${name.padEnd(11)} ${ok}/${fixture.cases.length}`);
}
impl.close?.();
if (failed) die(`edit-fixtures: ${failed} of ${passed + failed} failed`);

function fixturePath(name) {
  return join(root, "languages", name, "tests", "edits.json");
}

function die(message) {
  console.error(message);
  process.exit(1);
}

/** Why the case fails, or null when every step comes out as expected. */
async function runCase(dialect, c) {
  let source = c.source;
  for (const [i, step] of c.steps.entries()) {
    const got = await impl.edit(dialect, source, step.op, step.args);
    const want = step.expect;
    const same = got.ok === want.ok &&
      (got.ok ? want.source === undefined || got.source === want.source : want.status === undefined || got.status === want.status);
    if (!same) {
      return [
        `  step ${i + 1}: ${step.op} ${JSON.stringify(step.args)}`,
        `  on       ${JSON.stringify(source)}`,
        `  expected ${show(want)}`,
        `  got      ${show(got)}`,
      ].join("\n");
    }
    if (got.ok) source = got.source;
  }
  return null;
}

function show(outcome) {
  return outcome.ok ? (outcome.source === undefined ? "ok" : JSON.stringify(outcome.source)) : `refusal ${outcome.status ?? "(any status)"}${outcome.message ? ` (${outcome.message})` : ""}`;
}

/** fig's own editor, through the TypeScript binding, with the WebAssembly
 *  module of each language under test registered. */
async function figImpl(names) {
  const fig = await import(pathToFileURL(join(root, "bindings", "typescript", "src", "index.ts")).href);
  for (const name of names) {
    const module = join(root, "languages", name, "js", `${name}.js`);
    if (!existsSync(module)) {
      die(`edit-fixtures: no languages/${name}/js/${name}.js; build it with \`zig build wasm && node js/build.mjs\` in languages/${name}`);
    }
    fig.registerLanguage((await import(pathToFileURL(module).href)).default);
  }
  const statusName = Object.fromEntries(Object.entries(fig.Status).map(([k, v]) => [v, k]));
  const refusal = (e) => {
    if (!(e instanceof fig.FigError)) throw e;
    return { ok: false, status: statusName[e.status] ?? `status ${e.status}`, message: e.message };
  };
  return {
    async edit(dialect, source, op, args) {
      let ed;
      try {
        ed = fig.Editor.open(source, fig.formatByName(dialect));
      } catch (e) {
        return refusal(e);
      }
      try {
        if (typeof ed[op] !== "function") throw new Error(`fig's Editor has no op ${op}`);
        ed[op](...args);
        return { ok: true, source: ed.source() };
      } catch (e) {
        return refusal(e);
      } finally {
        ed.dispose();
      }
    },
  };
}

/** Any implementation, as a command speaking JSON lines: one request per line
 *  on its stdin, one answer per line on its stdout, asked one at a time. */
async function commandImpl(command) {
  const child = spawn(command, { shell: true, stdio: ["pipe", "pipe", "inherit"] });
  let buffered = "", next = 0, waiting = null;
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffered += chunk;
    let nl;
    while ((nl = buffered.indexOf("\n")) >= 0) {
      const line = buffered.slice(0, nl);
      buffered = buffered.slice(nl + 1);
      if (line.trim() && waiting) waiting(line);
    }
  });
  child.on("exit", (code, signal) => {
    if (waiting) die(`edit-fixtures: ${command} exited (${signal ?? code})`);
  });
  const ask = (method, params) => new Promise((resolve) => {
    const id = next++;
    waiting = (line) => {
      waiting = null;
      const msg = JSON.parse(line);
      if (msg.id !== id) die(`edit-fixtures: ${command} answered ${JSON.stringify(msg.id)}, asked ${id}`);
      resolve(msg);
    };
    child.stdin.write(JSON.stringify({ id, method, ...params }) + "\n");
  });
  const hello = await ask("hello", { protocol: 1 });
  if (!(hello.methods ?? []).includes("edit")) die(`edit-fixtures: ${command} has no edit method`);
  return {
    async edit(dialect, source, op, args) {
      const res = await ask("edit", { dialect, source, op, args });
      return res.ok ? { ok: true, source: res.source } : { ok: false, status: res.status, message: res.error };
    },
    close: () => child.stdin.end(),
  };
}
