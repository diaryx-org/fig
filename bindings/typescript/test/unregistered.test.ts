// The module as a caller first meets it: no format compiled in and none
// registered yet. What is not registered says which language to import,
// and registering is idempotent, so a library and the application using it
// may each register the languages they import.
import { test } from "node:test";
import assert from "node:assert/strict";

import { Document, FigError, Status, convert, formatByName, formatName, registerLanguage, requireFormat, stringify } from "../src/index.ts";
import json from "@diaryx/fig-json";
import yaml from "@diaryx/fig-yaml";

/** `f` throws the unregistered-format error naming `module`. */
function refusesFor(f: () => unknown, module: string): void {
  assert.throws(f, (err: unknown) => {
    assert.ok(err instanceof FigError, String(err));
    assert.equal(err.status, Status.UnsupportedFormat);
    assert.ok(
      err.message.includes(`import ${module} from "@diaryx/fig-${module}"`) && err.message.includes("registerLanguage"),
      err.message,
    );
    return true;
  });
}

test("a format no language is registered for says which one to import", () => {
  assert.equal(formatByName("toml"), null);
  refusesFor(() => requireFormat("toml"), "toml");
  // JSONC is a dialect of the JSON language, and YAML 1.1 of YAML's.
  refusesFor(() => requireFormat("jsonc"), "json");
  refusesFor(() => requireFormat("yaml-1.1"), "yaml");
  // A name no package language has gets no module to import.
  assert.throws(
    () => requireFormat("no-such"),
    (err: unknown) => err instanceof FigError && err.status === Status.UnsupportedFormat && !err.message.includes("@diaryx/fig-") && /registerLanguage/.test(err.message),
  );
  // A number no registration handed out is not a format.
  assert.throws(
    () => Document.parse("a = 1\n", 7 as never),
    (err: unknown) => err instanceof FigError && err.status === Status.UnsupportedFormat && /names no registered format/.test(err.message),
  );
  assert.throws(() => stringify({ a: 1 }, 0 as never), (err: unknown) => err instanceof FigError && err.status === Status.UnsupportedFormat);
});

test("registering a language brings in its format, and registering it again is the same format", () => {
  const first = registerLanguage(yaml);
  // Handles are assigned in registration order, from 1.
  assert.equal(first, 1);
  assert.equal(yaml.format, first);
  assert.equal(formatByName("yaml"), first);
  assert.equal(formatName(first), "yaml");
  assert.equal(registerLanguage(yaml), first);
  const Json = registerLanguage(json);
  assert.equal(json.format, Json);
  assert.equal(convert("a: [1, 2]\n", first, Json), '{\n  "a": [\n    1,\n    2\n  ]\n}\n');
  assert.equal(convert("// c\n{\"a\": 1}\n", requireFormat("jsonc"), first), "# c\na: 1\n");
  // A different language under a name already taken is refused as ever.
  assert.throws(() => registerLanguage({ ...yaml }), (err: unknown) => err instanceof FigError && /already registered/.test(err.message));
});
