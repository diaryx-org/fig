// Public enums and the error type, shared across the binding.
//
// ENUMS. Every enum in this binding is a frozen-shape object literal
// (`as const`) paired with a type of the same name — `NodeKind.Mapping` is a
// value, `NodeKind` a type — rather than a TypeScript `enum`. The pair is erasable
// syntax: Node's strip-only TypeScript and `erasableSyntaxOnly` accept it, and
// the compiled JavaScript is the object literal itself.
//
// UNKNOWN VALUES — the one rule, which every enum mirroring a C enum follows
// (`NodeKind`, `ExtKind`, `WarningCode`, `WarningCause`, `Status`): a number
// the core reports that has no name here passes through unchanged and
// compares numerically. Nothing maps it to a name, throws on it, or clamps
// it. That is what a node kind, extended kind, warning or status added by a
// newer core looks like to an older binding. So each such type is its
// named members *or any number*, and code that switches on one keeps a
// `default` arm. The member names are for writing; the number is the value.
import { Status } from "./ffi.ts";

export { Status };

/** A config format: the opaque handle of a registered dialect. The module
 *  compiles no format in, so every format is one a language brought —
 *  `registerLanguage(yaml)` returns YAML's, as `yaml.format` holds it from
 *  then on — and the rest of a language's dialects are found by name with
 *  {@link formatByName} (`"jsonc"`, `"yaml-1.1"`). Handles are assigned per
 *  process in registration order, so persist the name, never the number.
 *  Through 5.x this was also an object of fixed members (`Format.Json`);
 *  6.0 has none. */
export type { Format } from "./wire.ts";

/** Controls how {@link stringify}, `convert` and `Document.serialize` render
 *  output. Omitted fields fall back to
 *  fig's historical style (pretty-printed, two-space indent), so passing no
 *  options renders exactly as before. Each language reads the options it has a
 *  use for: of the package's, `pretty` is honored by JSON (multi-line vs.
 *  minified), ZON (`zig fmt` multi-line vs. inline `.{ a, b }`), and TOML
 *  (gates array wrapping); `indent` by JSON and TOML's wrapped arrays;
 *  `width` by the inline-vs-expanded layout of TOML, YAML, and fig. */
export interface SerializeOptions {
  /** `true` (default): multi-line, indented output. `false`: compact
   *  single-line output with no insignificant whitespace. For TOML, `false`
   *  keeps every array on one line; `true` lets a wide array wrap (see `width`). */
  pretty?: boolean;
  /** Spaces per indentation level when `pretty` (JSON, and TOML's wrapped
   *  arrays). Defaults to 2. */
  indent?: number;
  /** Drop comments carried on the value instead of emitting them. Defaults to
   *  `false` (preserve where the target format allows). */
  stripComments?: boolean;
  /** `Document.serialize` only: preserve values the target cannot represent
   *  natively (a null in TOML, a TOML datetime in JSON, …) through a `$fig`
   *  envelope, and decode any such envelope in the source. Defaults to `false`
   *  (lossy — an unrepresentable value throws `UnsupportedFormat`). Ignored by
   *  `stringify` (a built value has no source envelopes). */
  lossless?: boolean;
  /** The column budget for the inline-vs-expanded layout of TOML, YAML and
   *  fig. A mapping/array that renders within
   *  `width` columns stays inline (`k = { … }` / `[a, b]`); a wider one expands
   *  to a `[section]` / a wrapped array / block lines. Defaults to `80`. Ignored
   *  by the other formats.
   *
   *  Two limits before reaching for this as a "render block" lever: `0` means
   *  *unset* (it resolves to the default `80`, since zero-initialized option
   *  structs are ordinary across the C ABI) — pass `1` to force block; and for
   *  YAML the budget governs NESTED containers only, as a root mapping/sequence
   *  always renders block. */
  width?: number;
}

/** The kind of an AST node reached during read-path traversal. Mirrors
 *  `FigNodeKind`; an unnamed value is a kind from a newer core (see the
 *  unknown-values rule at the top of this file). */
export const NodeKind = {
  Invalid: -1,
  Null: 0,
  Bool: 1,
  Int: 2,
  Float: 3,
  String: 4,
  Sequence: 5,
  Mapping: 6,
  KeyValue: 7,
  Alias: 8,
  /** A format-specific scalar; `Document.asExtended` says which. */
  Extended: 9,
} as const;
export type NodeKind = (typeof NodeKind)[keyof typeof NodeKind] | (number & {});

/** A format-specific scalar kind (TOML datetimes, ZON enum/char literals, JSON5
 *  non-finite numbers, plist dates and data). Mirrors `FigExtKind`; unknown
 *  values follow the rule at the top of this file. */
export const ExtKind = {
  OffsetDateTime: 0,
  LocalDateTime: 1,
  LocalDate: 2,
  LocalTime: 3,
  EnumLiteral: 4,
  CharLiteral: 5,
  /** A non-finite JSON5 number (`Infinity`/`-Infinity`/`NaN`). */
  NumberSpecial: 6,
  /** A plist `<date>`: the raw ISO-8601 timestamp, verbatim. */
  PlistDate: 7,
  /**
   * A byte string. Its text is always base64 (standard alphabet, padded, no
   * whitespace): a plist `<data>` or YAML `!!binary` writes that base64, and a
   * binary format holds the raw bytes, which its language encodes and decodes.
   */
  Data: 8,
} as const;
export type ExtKind = (typeof ExtKind)[keyof typeof ExtKind] | (number & {});

/** What kind of loss a {@link Warning} describes. Mirrors `FigWarningCode`. */
export const WarningCode = {
  /** A carried comment is not emitted at all. */
  CommentDropped: 0,
  /** A block comment is rendered as a run of line comments. */
  CommentStyleDegraded: 1,
  /** A node is removed entirely (the target cannot represent it even degraded). */
  ValueDropped: 2,
  /** An extended/non-finite value is rendered as a poorer type. */
  TypeDegraded: 3,
} as const;
export type WarningCode = (typeof WarningCode)[keyof typeof WarningCode] | (number & {});

/** Why a {@link Warning}'s loss happens. Mirrors `FigWarningCause`. */
export const WarningCause = {
  /** The target format inherently cannot represent it. */
  FormatLimitation: 0,
  /** A caller option forced it (e.g. `stripComments`). */
  ExplicitOption: 1,
} as const;
export type WarningCause = (typeof WarningCause)[keyof typeof WarningCause] | (number & {});

/** One lossy event reported by `Document.diagnose` / value `diagnose`. `code`
 *  and `cause` carry the raw ABI value; one with no name above follows the
 *  unknown-values rule at the top of this file. */
export interface Warning {
  code: WarningCode;
  cause: WarningCause;
  /** Dotted / `[i]` location; empty for the document root. */
  path: string;
  /** Degraded-to type for {@link WarningCode.TypeDegraded}, else empty. */
  note: string;
}

const STATUS_MESSAGE: Record<number, string> = {
  [Status.InvalidArgument]: "invalid argument",
  [Status.ParseError]: "parse error",
  [Status.OutOfMemory]: "out of memory",
  [Status.UnsupportedFormat]: "unsupported format",
  [Status.NotFound]: "not found",
  [Status.UnsupportedOperation]: "unsupported operation",
  [Status.InternalError]: "internal error",
};

/** Extra detail from a failure, as the core reports it in a `FigError` struct.
 *
 *  `message` is the core's one-line diagnostic. `byteOffset` locates the
 *  failure in the input, and is reported today by a runtime language's
 *  refusal (a `LanguageError`'s offset) — the compiled parsers report a
 *  message and no location yet. `line`/`column` (1-based) are present only
 *  when the core reports them.
 *
 *  An absent field is `undefined`, never 0. The C struct itself spells
 *  "unknown" as 0 in all three fields, which is unambiguous for the 1-based
 *  `line`/`column` but not for `byteOffset`: a refusal at the very first byte
 *  reaches this binding exactly as "no offset" does, and so surfaces as
 *  `undefined`. */
export interface ParseDetail {
  message?: string | undefined;
  byteOffset?: number | undefined;
  line?: number | undefined;
  column?: number | undefined;
}

/** An error carrying the originating fig {@link Status} code and the
 *  operation that failed, plus — for parse failures — the core's message and
 *  source location when available. `message` reads `<op>: <what>` with the
 *  location appended; the parts are also fields, so nothing has to be parsed
 *  back out of it. */
export class FigError extends Error {
  readonly status: Status;
  /** The operation that failed — a C ABI entry point (`fig_parse`,
   *  `fig_editor_create`) or a method name (`renameKey`, `moveContainer`). */
  readonly op: string;
  readonly byteOffset?: number | undefined;
  readonly line?: number | undefined;
  readonly column?: number | undefined;
  constructor(status: Status, op: string, detail?: ParseDetail) {
    const base = detail?.message && detail.message.length > 0
      ? detail.message
      : (STATUS_MESSAGE[status] ?? `status ${status}`);
    const loc = detail?.line != null && detail?.column != null
      ? ` (line ${detail.line}, column ${detail.column})`
      : detail?.byteOffset != null
        ? ` (byte offset ${detail.byteOffset})`
        : "";
    super((op ? `${op}: ${base}` : base) + loc);
    this.name = "FigError";
    this.status = status;
    this.op = op;
    this.byteOffset = detail?.byteOffset;
    this.line = detail?.line;
    this.column = detail?.column;
  }
}

/** Throw a {@link FigError} unless `status` is `Ok`. */
export function check(status: number, op: string): void {
  if (status !== Status.Ok) throw new FigError(status, op);
}

/** The {@link FigError} a handle throws when used after `dispose()`: a
 *  malformed call, so `InvalidArgument`. */
export function disposedError(what: string): FigError {
  return new FigError(Status.InvalidArgument, what, { message: "already disposed" });
}

