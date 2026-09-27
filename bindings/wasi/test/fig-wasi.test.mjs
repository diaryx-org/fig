// End-to-end tests: spawn the packaged bin/fig.mjs exactly as npx would, and
// drive it against real temp files. This is what actually exercises the WASI
// rights-compatibility shim in bin/fig.mjs — a unit test that mocked
// node:wasi would miss the real bug (ENOTCAPABLE on every file open) that
// shim works around.
//
// Requires wasm/fig-wasi.wasm to already exist — run `npm run build` first
// (release.yml and prepublishOnly both do this before `npm test`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const bin = join(here, "..", "bin", "fig.mjs");

function run(args, opts = {}) {
  return execFileSync(process.execPath, [bin, ...args], { encoding: "utf8", ...opts });
}

function withTempDir(fn) {
  const dir = mkdtempSync(join(tmpdir(), "fig-wasi-"));
  try {
    fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// Format is `fig <version> "<epoch>"` — see src/cli/actions.zig's runVersion
// and docs/VERSIONING.md. Every artifact ships under fig's one version, so the
// module this package carries has to report the package's own: a wasm left
// over from another release fails here rather than on someone's machine.
const pkgVersion = JSON.parse(readFileSync(join(here, "..", "package.json"), "utf8")).version;
const VERSION_RE = new RegExp(`^fig ${pkgVersion.replaceAll(".", "\\.")} "[^"]+"$`);

test("version prints the package's version and the epoch", () => {
  const out = run(["version"]);
  assert.match(out.trim(), VERSION_RE);
});

test("get converts YAML to JSON", () => {
  withTempDir((dir) => {
    const file = join(dir, "test.yaml");
    writeFileSync(file, "name: fig\nport: 8080\n");
    const out = run(["get", "-o", "json", "test.yaml"], { cwd: dir });
    assert.deepEqual(JSON.parse(out), { name: "fig", port: 8080 });
  });
});

test("set edits a file in place, preserving the rest byte-for-byte", () => {
  withTempDir((dir) => {
    const file = join(dir, "test.yaml");
    writeFileSync(file, "# keep me\nname: fig\n");
    run(["set", "test.yaml", "port", "9090"], { cwd: dir });
    const contents = readFileSync(file, "utf8");
    assert.match(contents, /# keep me/);
    assert.match(contents, /port: 9090/);
  });
});

test("relative paths resolve from a subdirectory (the '.' preopen)", () => {
  withTempDir((dir) => {
    mkdirSync(join(dir, "sub"));
    writeFileSync(join(dir, "sub", "x.json"), '{"a":1}');
    const out = run(["get", "sub/x.json"], { cwd: dir });
    assert.deepEqual(JSON.parse(out), { a: 1 });
  });
});

// The module compiles no format in: every one is a language of
// `@diaryx/fig`, which bin/fig.mjs serves when the CLI asks for it by name
// or by extension (src/cli/host_languages.zig).
test("every format is served, by its extension", () => {
  const files = {
    "a.json": '{"k": 1}',
    "a.jsonc": '// c\n{"k": 1}',
    "a.json5": "{k: 1}",
    "a.yaml": "k: 1\n",
    "a.yml": "k: 1\n",
    "a.toml": "k = 1\n",
    "a.zon": ".{ .k = 1 }\n",
    "a.figl": "k = 1\n",
    "a.ini": "k = 1\n",
    "a.env": "k=1\n",
    "a.properties": "k=1\n",
    "a.nt": "k: 1\n",
    "a.plist": '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0">\n<dict>\n  <key>k</key>\n  <integer>1</integer>\n</dict>\n</plist>\n',
  };
  withTempDir((dir) => {
    for (const [name, text] of Object.entries(files)) {
      writeFileSync(join(dir, name), text);
      assert.equal(run(["get", name, "k"], { cwd: dir }), "1\n", name);
    }
  });
});

test("a value argument is read as fig reads it, and frontmatter is edited in place", () => {
  withTempDir((dir) => {
    writeFileSync(join(dir, "c.yaml"), "a: 1\n");
    run(["set", "c.yaml", "a", "2"], { cwd: dir });
    run(["set", "c.yaml", "b", "[x, y]"], { cwd: dir });
    assert.equal(readFileSync(join(dir, "c.yaml"), "utf8"), "a: 2\nb:\n- x\n- y\n");
    writeFileSync(join(dir, "n.md"), "---\ntitle: Hi\n---\nbody\n");
    run(["set", "n.md", "title", "Bye"], { cwd: dir });
    assert.equal(readFileSync(join(dir, "n.md"), "utf8"), "---\ntitle: Bye\n---\nbody\n");
  });
});

test("a file with no extension it knows is sniffed through the languages", () => {
  withTempDir((dir) => {
    writeFileSync(join(dir, "config"), "[server]\nport = 8080\n");
    assert.deepEqual(JSON.parse(run(["get", "config", "-o", "json"], { cwd: dir })), { server: { port: 8080 } });
  });
});

test("stdin is read in the format named, and a parse error names where", () => {
  assert.equal(run(["get", "-i", "yaml", "-", "-o", "toml"], { input: "a: 1\n" }), "a = 1\n");
  withTempDir((dir) => {
    writeFileSync(join(dir, "bad.json"), '{"a": }');
    assert.throws(
      () => run(["get", "bad.json"], { cwd: dir, stdio: "pipe" }),
      (err) => err.status === 1 && /bad\.json:1:/.test(err.stderr),
    );
  });
});

test("check validates a file and exits 0", () => {
  withTempDir((dir) => {
    writeFileSync(join(dir, "ok.json"), "{}");
    assert.doesNotThrow(() => run(["check", "ok.json"], { cwd: dir }));
  });
});

test("a missing file exits non-zero with fig's own clean error, not a JS stack trace", () => {
  withTempDir((dir) => {
    try {
      run(["get", "nope.yaml"], { cwd: dir });
      assert.fail("expected a non-zero exit");
    } catch (err) {
      assert.equal(err.status, 1);
      // What this asserts is the SHAPE of fig's own diagnostic — its `error: `
      // prefix at the start of a line, naming the offending path — not the exact
      // wording of the reason. The wording is deliberately not pinned: this
      // assertion was written as /error: FileNotFound/ (the raw Zig error name),
      // and when `dbc8dfc` replaced those names with human messages it started
      // matching nothing. Because the fig-wasi suite runs only at publish time
      // (`prepublishOnly`) and not in CI, that surfaced as a failed *release*
      // job, twice, rather than as a failed build.
      assert.match(err.stderr.toString(), /^error: .*nope\.yaml/m);
      assert.doesNotMatch(err.stderr.toString(), /at file:\/\//); // no raw JS stack trace
    }
  });
});

// Regression test: a command that never touches stdin (`version`) must not
// hang when stdin happens to be a pipe that's never written to or closed —
// e.g. an interactive shell whose own stdin is presented as a pipe rather
// than a raw TTY (common inside another program, an IDE-integrated
// terminal, tmux, ...). bin/fig.mjs used to eagerly drain any pipe-like
// stdin before starting the guest (to work around node:wasi's ESPIPE bug on
// non-seekable fds — see the "stdio compatibility shim" comment there), which
// hung forever whenever nothing ever closed that pipe. The fix gates the
// eager read on whether `-` was actually passed. `execFileSync` can't express
// this (it can't leave stdin open-but-unwritten while asserting on a
// timeout), hence the manual `spawn` + race here.
test("does not hang when stdin is an open pipe that is never closed and never needed", async () => {
  const child = spawn(process.execPath, [bin, "version"], { stdio: ["pipe", "pipe", "pipe"] });
  // Deliberately never write to or close child.stdin.
  let out = "";
  child.stdout.on("data", (d) => (out += d));
  try {
    const result = await Promise.race([
      new Promise((resolve) => child.on("exit", (code) => resolve({ code }))),
      new Promise((_, reject) => setTimeout(() => reject(new Error("timed out: hung waiting on unrelated stdin")), 5000)),
    ]);
    assert.equal(result.code, 0);
    assert.match(out.trim(), VERSION_RE);
  } finally {
    child.kill();
  }
});
