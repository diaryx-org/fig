// Build the fig CLI as a WASI module and vendor it into this package.
//
// Runs `zig build wasi` at the repository root with no format compiled in
// and `-Dwasi-host=true` — so, unlike release-binaries.yml's standalone
// WASI artifact, this module asks its host for every format — then copies
// the result into wasm/fig-wasi.wasm, and the languages bin/fig.mjs serves
// into lib/.
//
// Unlike the sibling `@diaryx/fig` library package, this ships the
// module as a real file rather than inlining it as base64: there's no
// bundler to appease here, just a plain `fs.readFileSync` from bin/fig.mjs.
//
// Builds into its own `--prefix` (rather than the default zig-out/) so this
// doesn't collide with `bindings/typescript`'s `zig build wasm` output, or
// with `zig build wasi`'s own default output, when both are built from the
// same checkout.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { stripTypeScriptTypes } from "node:module";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const pkgDir = resolve(here, "..");
const prefix = join(repoRoot, "zig-out", "wasi-npm");

// No format compiled in, and the host asked for each: bin/fig.mjs serves
// `@diaryx/fig`'s JavaScript languages to the CLI in-process
// (src/cli/host_languages.zig), as the library package serves them to its
// own module.
const LANGUAGES = ["json", "yaml", "toml", "zon", "fig", "ini", "dotenv", "properties", "plist", "nestedtext"];
const zigArgs = ["build", "wasi", "-Dwasi-host=true", ...LANGUAGES.map((name) => `-D${name}=false`), "--prefix", prefix];
console.error(`· zig ${zigArgs.join(" ")}`);
execFileSync("zig", zigArgs, { cwd: repoRoot, stdio: "inherit" });

const wasmSrc = join(prefix, "bin", "fig-wasi.wasm");
const wasmDestDir = join(pkgDir, "wasm");
mkdirSync(wasmDestDir, { recursive: true });
copyFileSync(wasmSrc, join(wasmDestDir, "fig-wasi.wasm"));
console.error("· wrote wasm/fig-wasi.wasm");

// The languages, the kit they are written with, and the wire's `handle`,
// from the library package beside this one — its sources, so nothing there
// needs building: `src/wire.ts` imports nothing and is erasable syntax, so
// Node's own type stripping makes it the JavaScript `dist/wire.js` would be. Copied rather than depended on, as the wasm is:
// the two packages release together, and this one needs nothing of the
// other at install time. Each file names the others by package subpath
// (`@diaryx/fig/kit/grammar`), which is rewritten to a relative path here,
// since nothing installs `@diaryx/fig` beside this package.
const lib = resolve(pkgDir, "..", "typescript");
const out = join(pkgDir, "lib");
rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, "kit"), { recursive: true });
mkdirSync(join(out, "languages"), { recursive: true });

function relink(text, fromDir) {
  const to = (target) => {
    const rel = target.startsWith(fromDir + "/") ? "./" + target.slice(fromDir.length + 1) : "../" + target;
    return fromDir === "" ? "./" + target : rel;
  };
  return text
    .replace(/from "@diaryx\/fig\/helper"/g, `from "${to("wire.js")}"`)
    .replace(/from "@diaryx\/fig\/kit"/g, `from "${to("kit/index.js")}"`)
    .replace(/from "@diaryx\/fig\/kit\/(\w+)"/g, (_, m) => `from "${to(`kit/${m}.js`)}"`)
    .replace(/^\/\/# sourceMappingURL=.*$/m, "");
}

writeFileSync(join(out, "wire.js"), stripTypeScriptTypes(readFileSync(join(lib, "src", "wire.ts"), "utf8")));
for (const dir of ["kit", "languages"]) {
  for (const f of readdirSync(join(lib, dir)).filter((f) => f.endsWith(".js"))) {
    writeFileSync(join(out, dir, f), relink(readFileSync(join(lib, dir, f), "utf8"), dir));
  }
}
console.error("· wrote lib/ (the languages, the kit and the wire, from ../typescript)");
