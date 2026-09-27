// Library introspection: version and per-format capabilities of the linked
// (here, bundled-wasm) fig core.
import { fig, readCString } from "./ffi.ts";
import { FigError, Format, Status } from "./types.ts";

/** The bundled fig core's version. */
export interface Version {
  major: number;
  minor: number;
  patch: number;
}

/** The bundled fig core's version, decoded from the packed
 *  `(major << 16) | (minor << 8) | patch` that `fig_version` returns. */
export function version(): Version {
  const packed = fig.fig_version() >>> 0;
  return { major: (packed >>> 16) & 0xff, minor: (packed >>> 8) & 0xff, patch: packed & 0xff };
}

/** The bundled fig core's version as a `"major.minor.patch"` string. */
export function versionString(): string {
  return readCString(fig.fig_version_string());
}

/** What the module can do with a format. The published module compiles no
 *  format in, so a format reports all-`false` until a language for it is
 *  registered ({@link registerLanguage}), and then the `caps` that language
 *  declared. `references` is a property of the format: YAML has it. */
export interface Capabilities {
  /** `Document.parse` accepts this format. */
  read: boolean;
  /** The editor / embed APIs accept this format. */
  edit: boolean;
  /** The serializers can write this format. */
  serialize: boolean;
  /** The format has a reference layer — anchors, aliases, `<<` merges, tags
   *  (YAML's). A document leaving such a format for one without is collapsed
   *  first; one written to a format that has the layer too keeps it. */
  references: boolean;
}

/** Query what this build can do with `format` (read / edit / serialize /
 *  references), so a host can pick a working format up front instead of
 *  probing for errors. */
export function capabilities(format: Format): Capabilities {
  const bits = fig.fig_format_capabilities(format) >>> 0;
  return {
    read: (bits & 1) !== 0,
    edit: (bits & 2) !== 0,
    serialize: (bits & 4) !== 0,
    references: (bits & 8) !== 0,
  };
}

/** The module of `@diaryx/fig/languages` that serves each format the
 *  package names: JSONC is a dialect of the JSON5 language. */
const LANGUAGE_MODULE: Record<number, string> = {
  [Format.Json]: "json",
  [Format.Jsonc]: "json5",
  [Format.Json5]: "json5",
  [Format.Yaml]: "yaml",
  [Format.Toml]: "toml",
  [Format.Zon]: "zon",
  [Format.Fig]: "fig",
  [Format.Ini]: "ini",
  [Format.Dotenv]: "dotenv",
  [Format.Properties]: "properties",
  [Format.Plist]: "plist",
  [Format.Nestedtext]: "nestedtext",
};

/** Throw for a failed call's `status`, saying which language to register
 *  when the failure is `format` having none: the module compiles no format
 *  in, and a caller that has not registered one meets this first. */
export function checkFormat(status: number, op: string, format: Format): void {
  if (status === Status.Ok) return;
  const module = LANGUAGE_MODULE[format];
  if (status === Status.UnsupportedFormat && module !== undefined && !capabilities(format).read) {
    throw new FigError(status, op, {
      message: `no language is registered for this format — import ${module} from "@diaryx/fig/languages/${module}" and pass it to registerLanguage()`,
    });
  }
  throw new FigError(status, op);
}
