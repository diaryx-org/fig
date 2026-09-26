// Build step, after tsc: take the source maps off the inlined wasm payload.
//
// `src/wasm-bytes.ts` is generated (build-wasm.mjs) and holds nothing but the
// base64 module, so its maps map nothing worth reading, and `files` leaves the
// source and both maps out of the tarball. tsc still ends `dist/wasm-bytes.js`
// and `.d.ts` with a `sourceMappingURL` comment, which would point a bundler's
// source-map loader at a file the installed package does not have. So both
// maps are deleted and both comments removed, and the published files refer
// to nothing that is not there.
import { existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const dist = join(dirname(dirname(fileURLToPath(import.meta.url))), 'dist');

for (const file of ['wasm-bytes.js', 'wasm-bytes.d.ts']) {
  const path = join(dist, file);
  if (!existsSync(path)) continue;
  const text = readFileSync(path, 'utf8');
  writeFileSync(path, text.replace(/\n\/\/# sourceMappingURL=\S+\s*$/, '\n'));
  rmSync(path + '.map', { force: true });
}
