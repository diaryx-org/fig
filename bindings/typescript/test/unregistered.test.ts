// The module as a caller first meets it: no format compiled in and none
// registered yet. What is not registered says which language to import,
// and registering is idempotent, so a library and the application using it
// may each register the languages they import.
import { test } from "node:test";
import assert from "node:assert/strict";

import { Document, Editor, Embed, EmbedType, FigError, Format, Status, capabilities, convert, registerLanguage, stringify } from "../src/index.ts";
import json from "@diaryx/fig/languages/json";
import json5 from "@diaryx/fig/languages/json5";
import yaml from "@diaryx/fig/languages/yaml";

/** `f` throws the unregistered-format error naming `module`. */
function refusesFor(f: () => unknown, module: string): void {
  assert.throws(f, (err: unknown) => {
    assert.ok(err instanceof FigError, String(err));
    assert.equal(err.status, Status.UnsupportedFormat);
    assert.ok(
      err.message.includes(`import ${module} from "@diaryx/fig/languages/${module}"`) && err.message.includes("registerLanguage"),
      err.message,
    );
    return true;
  });
}

test("a format no language is registered for says which one to import", () => {
  assert.deepEqual(capabilities(Format.Toml), { read: false, edit: false, serialize: false, references: false });
  refusesFor(() => Document.parse("a = 1\n", Format.Toml), "toml");
  refusesFor(() => Editor.open("a = 1\n", Format.Toml), "toml");
  refusesFor(() => Embed.open("+++\na = 1\n+++\n", EmbedType.Plus), "toml");
  refusesFor(() => stringify({ a: 1 }, Format.Toml), "toml");
  // JSONC is a dialect of the JSON5 language.
  refusesFor(() => Document.parse("{}", Format.Jsonc), "json5");
});

test("registering a language brings in its format, and registering it again is the same format", () => {
  const first = registerLanguage(yaml);
  assert.equal(first, Format.Yaml);
  assert.equal(registerLanguage(yaml), first);
  registerLanguage(json);
  registerLanguage(json5);
  assert.equal(convert("a: [1, 2]\n", Format.Yaml, Format.Json), '{\n  "a": [\n    1,\n    2\n  ]\n}\n');
  assert.equal(convert("// c\n{\"a\": 1}\n", Format.Jsonc, Format.Yaml), "# c\na: 1\n");
  // A different language under a name already taken is refused as ever.
  assert.throws(() => registerLanguage({ ...yaml }), (err: unknown) => err instanceof FigError && /already registered/.test(err.message));
  // Frontmatter reaches the registered YAML.
  using fm = Embed.open("---\ntitle: Hi\n---\nbody\n", EmbedType.Frontmatter);
  fm.setValue(["title"], "Bye");
  assert.equal(fm.render(), "---\ntitle: Bye\n---\nbody\n");
});
