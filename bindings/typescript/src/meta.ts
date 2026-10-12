// Library introspection: version and per-format capabilities of the linked
// (here, bundled-wasm) fig core.
import { Frame, fig, readCString } from "./ffi.ts";
import { FigError, Status, type Format } from "./types.ts";

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

/** The module of `@diaryx/fig/languages` that registers each dialect the
 *  package's languages name: JSONC is a dialect of the JSON5 language, and
 *  YAML 1.1 one of YAML's. */
const LANGUAGE_MODULE: Readonly<Record<string, string>> = {
  json: "json",
  jsonc: "json5",
  json5: "json5",
  yaml: "yaml",
  "yaml-1.1": "yaml",
  toml: "toml",
  zon: "zon",
  fig: "fig",
  ini: "ini",
  dotenv: "dotenv",
  properties: "properties",
  plist: "plist",
  nestedtext: "nestedtext",
};

/** The error for a dialect `name` no registered language has: which module
 *  to import, where the package ships one. */
export function unregisteredError(name: string, op: string): FigError {
  const module = Object.hasOwn(LANGUAGE_MODULE, name) ? LANGUAGE_MODULE[name] : undefined;
  return new FigError(Status.UnsupportedFormat, op, {
    message:
      module === undefined
        ? `no language is registered for the format ${JSON.stringify(name)} — pass the language that has it to registerLanguage()`
        : `no language is registered for the format ${JSON.stringify(name)} — import ${module} from "@diaryx/fig/languages/${module}" and pass it to registerLanguage()`,
  });
}

/** The name of the dialect `format` names, or `null` for a number no
 *  registration handed out. */
export function formatName(format: Format): string | null {
  const ptr = fig.fig_format_name(format) >>> 0;
  return ptr === 0 ? null : readCString(ptr);
}

/** The `Format` of the dialect named `name`, or `null` when no registered
 *  language has one by that name. The only stable way to persist a format:
 *  its handle is assigned per process, its name is not. */
export function formatByName(name: string): Format | null {
  const frame = new Frame();
  try {
    const s = frame.str(name + "\0");
    const value = fig.fig_format_by_name(s.ptr);
    return value <= 0 ? null : (value as Format);
  } finally {
    frame.dispose();
  }
}

/** {@link formatByName}, throwing a {@link FigError} with
 *  `Status.UnsupportedFormat` where it would answer `null` — whose message,
 *  for a dialect one of the package's languages has, names the
 *  `@diaryx/fig/languages/<module>` to import and register. */
export function requireFormat(name: string): Format {
  const format = formatByName(name);
  if (format === null) throw unregisteredError(name, `requireFormat(${JSON.stringify(name)})`);
  return format;
}

/** Throw for a failed call's `status`. `UnsupportedFormat` for a number no
 *  registration handed out says so, since the module compiles no format in
 *  and every format is the handle of a registered dialect. */
export function checkFormat(status: number, op: string, format: Format): void {
  if (status === Status.Ok) return;
  if (status === Status.UnsupportedFormat && (typeof format !== "number" || formatName(format) === null)) {
    throw new FigError(status, op, {
      message: `${String(format)} names no registered format — a format is what registerLanguage() returns or formatByName() finds`,
    });
  }
  throw new FigError(status, op);
}
