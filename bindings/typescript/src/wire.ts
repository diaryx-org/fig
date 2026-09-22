// The helper wire, in JavaScript: what a format written in JavaScript is
// (`Language`), and one request line to one response line (`handle`).
//
// The contract is fig's helper wire — the newline-delimited JSON a `fig`
// command line speaks to a helper process, documented once on the `helper`
// module of the Rust crate (bindings/rust/fig/src/helper.rs). A `Language`
// here is that wire's `description`, with the functions the format is:
// `parse` answers the wire's node table, `print` is handed one back, and
// `render` spells the fragments the editor needs. Field names are the wire's
// own (`max_mapping_depth`, `empty_doc_seed`, `ext_kind`) rather than
// camelCase, on purpose: the object IS the wire document, so what
// `fig lang table <file>` prints is exactly what a `parse` must return, and
// the same object serves a `fig` CLI as a helper process through `serve`
// with no translation.
//
// This module imports nothing from the wasm side — no `ffi`, no module
// bytes — and must stay that way: it is what `@diaryx/fig/helper` resolves
// to, so a helper process loads the wire and its language and nothing else,
// and a runner that embeds a JavaScript engine can vendor this one file and
// have the same `handle` the wasm module calls. `registerLanguage`, which
// needs the module, is in `language.ts`.

// ── the description ────────────────────────────────────────────────────────

/** How one comment is delimited: `open` alone (`#`, `//`) or a pair
 *  (`<!--`, `-->`). `forbidden` is text a comment body may not contain. */
export interface CommentDelimiter {
  open?: string | null;
  close?: string | null;
  forbidden?: string | null;
}

/** A format's comment surface. `style` selects the owned-comment-block
 *  scanner; `line` is the own-line delimiter, `trailing` the same-line one. */
export interface Comments {
  style: "hash" | "slashes" | "semicolon" | "xml_comment";
  line?: CommentDelimiter | null;
  trailing?: CommentDelimiter | null;
}

/** How a section format spells a header line that opens a container. */
export interface SectionHeader {
  open?: string | null;
  close?: string | null;
  seq_open?: string | null;
  seq_close?: string | null;
  /** Joins path segments; default `.`. */
  sep?: string | null;
  /** Leave index segments out of the path. Default true. */
  skip_index?: boolean;
}

/** The self-closing spellings of an empty block container, where a format
 *  has them (plist's `<dict/>`, `<array/>`). */
export interface ClosedContainers {
  map_open?: string | null;
  map_close?: string | null;
  seq_open?: string | null;
  seq_close?: string | null;
}

/** What the generic splice engine needs to know about a format's surface
 *  syntax — the `Syntax` a compiled format declares, field for field. An
 *  omitted field takes its default; the defaults are the wire's. */
export interface Syntax {
  comments?: Comments | null;
  /** `null` means the engine never writes `key<sep>value` for this format,
   *  which then requires a `render("entry", …)`. */
  kv_sep?: string | null;
  flow_kv_sep_from_siblings?: boolean;
  flow_map_pad?: string | null;
  key_style?: "verbatim" | "json_quoted" | "zon_field" | "bare_or_quoted";
  /** A byte every key starts with, as its code (e.g. 46 for `.`), or 0. */
  key_sigil?: number;
  empty_map_literal?: string | null;
  /** Default true. */
  block_seq_editable?: boolean;
  /** Default true. */
  flow_containers?: boolean;
  indent_unit?: string | null;
  seq_item_marker?: string | null;
  closed_containers?: ClosedContainers | null;
  single_line_block_mapping?: boolean;
  /** Default true. */
  bare_document_mapping?: boolean;
  flow_map_open?: string | null;
  flow_map_close?: string | null;
  structural_indent?: boolean;
  section_noun?: "table" | "section" | "container" | null;
  section_header?: SectionHeader | null;
  merge_key?: string | null;
}

/** Which kinds the format holds natively — what the `$fig` lossless
 *  envelope need not wrap. */
export interface NativeKinds {
  null_?: boolean;
  offset_datetime?: boolean;
  local_datetime?: boolean;
  local_date?: boolean;
  local_time?: boolean;
  enum_literal?: boolean;
  char_literal?: boolean;
  number_special?: boolean;
  plist_date?: boolean;
  plist_data?: boolean;
}

/** One dialect of a language. The first row's `name` must be the
 *  language's, and is what {@link formatByName} resolves. */
export interface Dialect {
  name: string;
  extensions?: string[];
  /** How edit text is taken: as a literal, as a JSON string, or raw. */
  splice?: "literal" | "json_string" | "raw";
  /** What `set` writes to a file that does not exist yet; `null` refuses
   *  creation. */
  empty_doc_seed?: string | null;
  /** This dialect's own syntax where it differs from the language's. */
  syntax?: Syntax | null;
}

/** The five fragment renderers the editor may ask a format for. */
export type Renderer = "value" | "entry" | "item" | "tail" | "key";

/** What fig's bare-literal rules made of a value's text — classified once
 *  by fig so that every format means the same thing by `42`. */
export type Literal = "null" | "bool" | "int" | "float" | "datetime" | "string";

/** The arguments to a renderer. `indent` is the target line's indentation;
 *  `key`, `value` and `old_key` are as written; `literal` is set for the
 *  value renderer alone. */
export interface RenderArgs {
  dialect: string;
  indent: string;
  key: string;
  value: string;
  literal: Literal;
  old_key: string;
}

/** The subset of the serialize options a printer outside fig is told. */
export interface PrintOptions {
  pretty: boolean;
  strip_comments: boolean;
  indent: number;
  width: number;
  /** Print the value as the editor takes it spliced into a document, not as
   *  a document of its own: a root the document wraps (plist's `<plist>`) or
   *  a scalar root spelled differently from a scalar in place (NestedText's
   *  `>` block) is written bare. Every other print leaves it `false`. */
  splice: boolean;
}

// ── the node table ─────────────────────────────────────────────────────────

/** A `[start, end)` byte range into the input, 0-based. */
export type RowSpan = [number, number];

export type RowKind = "null" | "bool" | "int" | "float" | "string" | "sequence" | "mapping" | "keyvalue" | "alias";

export type RowExtKind =
  | "offset_datetime"
  | "local_datetime"
  | "local_date"
  | "local_time"
  | "enum_literal"
  | "char_literal"
  | "number_special"
  | "plist_date"
  | "plist_data";

/** One node. Rows are in pre-order — a parent precedes its children, a
 *  keyvalue is followed by its key row then its value row — and the row's
 *  index is its id. An absent optional is omitted, not `null`. */
export interface NodeRow {
  kind: RowKind;
  /** For an extended scalar: what tells it apart from the string or int
   *  `kind` reports. */
  ext_kind?: RowExtKind;
  /** The parent's row index; `null` for the root. */
  parent: number | null;
  /** Required of every row a parse returns; absent in a table fig builds
   *  for `print`. */
  span?: RowSpan;
  /** A scalar's decoded value; an int or float's lexeme; `true`/`false`
   *  for a bool; an alias's target anchor; an extended kind's payload.
   *  Absent for a container or null. */
  text?: string;
  anchor?: string;
  anchor_span?: RowSpan;
  tag?: string;
  tag_span?: RowSpan;
  /** For a block-sequence item: the `-`/`*` that introduces it. */
  marker?: RowSpan;
  /** For a keyvalue: the token separating key from value. A recorded
   *  separator says the value reframes rather than splices in place; a
   *  zero-width one marks a value hanging under a bare key. */
  sep?: RowSpan;
}

/** One whole header line of a section node (`[table]`), `end` just past
 *  the newline. A node with a region is a section. */
export interface RegionRow {
  node: number;
  start: number;
  end: number;
}

/** One place a section node's name is written. */
export interface MentionRow {
  node: number;
  span: RowSpan;
  kind: "header" | "entry";
}

/** One comment bound to a row. Grouped by node, in source order within a
 *  slot; at most one trailing per node. */
export interface CommentRow {
  node: number;
  slot: "leading" | "trailing" | "dangling";
  style: "line" | "block";
  text: string;
}

/** One tag-handle declaration — a YAML `%TAG` directive's handle (`!e!`, or
 *  a redefined `!`/`!!`) and the prefix it expands to. A tag spelled with a
 *  named handle is legal only in a document that declares it, so a parse
 *  returns those it read, in source order, and a print of a whole document
 *  (never of a fragment) receives them back to re-emit above any tag that
 *  uses one. */
export interface DirectiveRow {
  handle: string;
  prefix: string;
}

/** What `parse` returns and `print` receives: the tree, flat. Zero rows is
 *  refused — a format whose empty input is the empty document returns one
 *  `null` row. `regions`, `mentions`, `comments` and `directives` may be
 *  omitted when empty. */
export interface NodeTable {
  rows: NodeRow[];
  regions?: RegionRow[];
  mentions?: MentionRow[];
  comments?: CommentRow[];
  directives?: DirectiveRow[];
}

// ── the language ───────────────────────────────────────────────────────────

/** A format written in JavaScript: the wire's `description` — what the
 *  format declares — with the functions the format is. Hand one to
 *  {@link registerLanguage}, or {@link serve} it to a `fig` command line. */
export interface Language {
  /** Must differ from every compiled format's name and every language
   *  already registered; `dialects[0].name` must equal it. */
  name: string;
  /** `references`: the format has a reference layer — anchors, aliases,
   *  `<<` merges, tags — so its parse may return those columns and its print
   *  spells them; fig collapses the layer only when a document leaves such
   *  a format for one without. */
  caps: { read?: boolean; edit?: boolean; serialize?: boolean; references?: boolean };
  /** How deep a mapping may nest: `null` (or omitted) is unbounded, `0` a
   *  flat format holding no mapping inside its root. */
  max_mapping_depth?: number | null;
  /** Declared for the `$fig` lossless envelope; `null` means no envelope. */
  lossless?: NativeKinds | null;
  /** Required where `caps.edit` is set. */
  syntax?: Syntax | null;
  dialects: Dialect[];
  /** Required and non-empty: each is parsed, printed, reparsed and edited
   *  at registration, and a language whose samples fail is refused. */
  samples: string[];
  /** Which of `render`'s five fragments this format spells itself. */
  renderers?: Renderer[];

  /** The input — text; the wire carries no bytes that are not — to its
   *  node table. Refuse it by throwing a {@link LanguageError}, whose
   *  `byteOffset` is where; any other throw is reported without one. */
  parse(dialect: string, input: string): NodeTable;
  /** Required where `caps.serialize` is set. A table whose root is a scalar
   *  is a fragment the editor will splice: spell it as the scalar stands
   *  alone in the format. */
  print?(dialect: string, table: NodeTable, options: PrintOptions): string;
  /** Answer one of the renderers `renderers` declares. */
  render?(which: Renderer, args: RenderArgs): string;
}

/** How a language refuses its input: the message a caller sees, and the
 *  byte offset it points at, if any. */
export class LanguageError extends Error {
  readonly byteOffset: number | undefined;
  constructor(message: string, byteOffset?: number) {
    super(message);
    this.name = "LanguageError";
    this.byteOffset = byteOffset;
  }
}

// ── the wire ───────────────────────────────────────────────────────────────

/** A request as the wire spells it — what `handle` reads. */
type Request =
  | { op: "describe" }
  | { op: "parse"; dialect: string; input: string }
  | { op: "print"; dialect: string; table: NodeTable; options?: Partial<PrintOptions> }
  | {
      op: "render";
      which: Renderer;
      dialect: string;
      indent?: string;
      key?: string;
      value?: string;
      literal?: string;
      old_key?: string;
    };

/** The wire's `description` of `lang`: every declared field, spelled as the
 *  wire spells it. What `handle` answers `describe` with. */
export function describe(lang: Language): Record<string, unknown> {
  return {
    name: lang.name,
    caps: { read: !!lang.caps.read, edit: !!lang.caps.edit, serialize: !!lang.caps.serialize, references: !!lang.caps.references },
    max_mapping_depth: lang.max_mapping_depth ?? null,
    lossless: lang.lossless ?? null,
    syntax: lang.syntax ?? null,
    dialects: lang.dialects.map((d) => ({
      name: d.name,
      extensions: d.extensions ?? [],
      splice: d.splice ?? "literal",
      empty_doc_seed: d.empty_doc_seed ?? null,
      syntax: d.syntax ?? null,
    })),
    samples: lang.samples,
    renderers: lang.renderers ?? [],
  };
}

const RENDERERS: readonly Renderer[] = ["value", "entry", "item", "tail", "key"];
const LITERALS: readonly Literal[] = ["null", "bool", "int", "float", "datetime", "string"];

/** One request line to one response line — the helper wire, in
 *  JavaScript. What the wasm module calls for a registered language, and
 *  what {@link serve} runs over stdin and stdout. Never throws: a refusal,
 *  a malformed request and a bug in the language alike come back as
 *  `{"ok":false,"message":…}`. */
export function handle(lang: Language, requestLine: string): string {
  try {
    return JSON.stringify(handleInner(lang, requestLine));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const byteOffset = err instanceof LanguageError ? err.byteOffset : undefined;
    return JSON.stringify(byteOffset === undefined ? { ok: false, message } : { ok: false, message, byte_offset: byteOffset });
  }
}

function handleInner(lang: Language, requestLine: string): Record<string, unknown> {
  let req: Request;
  try {
    req = JSON.parse(requestLine) as Request;
  } catch (err) {
    throw new LanguageError(`request is not JSON: ${err instanceof Error ? err.message : String(err)}`);
  }
  if (typeof req !== "object" || req === null || typeof req.op !== "string") {
    throw new LanguageError("request has no op");
  }
  switch (req.op) {
    case "describe":
      return { ok: true, description: describe(lang) };
    case "parse": {
      if (typeof req.dialect !== "string" || typeof req.input !== "string") {
        throw new LanguageError("parse: dialect and input are strings");
      }
      const table = lang.parse(req.dialect, req.input);
      const out: Record<string, unknown> = { rows: table.rows, regions: table.regions ?? [], mentions: table.mentions ?? [], comments: table.comments ?? [] };
      if (table.directives && table.directives.length > 0) out.directives = table.directives;
      return { ok: true, table: out };
    }
    case "print": {
      if (!lang.print) throw new LanguageError(`${lang.name} does not serialize`);
      if (typeof req.dialect !== "string" || typeof req.table !== "object" || req.table === null || !Array.isArray(req.table.rows)) {
        throw new LanguageError("print: dialect is a string and table a node table");
      }
      const o = req.options ?? {};
      const options: PrintOptions = {
        pretty: o.pretty ?? true,
        strip_comments: o.strip_comments ?? false,
        indent: o.indent ?? 2,
        width: o.width ?? 80,
        splice: o.splice ?? false,
      };
      return { ok: true, output: lang.print(req.dialect, req.table, options) };
    }
    case "render": {
      if (!lang.render) throw new LanguageError(`${lang.name} declares no renderers`);
      if (!RENDERERS.includes(req.which)) throw new LanguageError(`render: unknown renderer ${JSON.stringify(req.which)}`);
      if (typeof req.dialect !== "string") throw new LanguageError("render: dialect is a string");
      // Absent or unknown: a string, which is what a renderer does with any
      // text it cannot type.
      const literal = LITERALS.includes(req.literal as Literal) ? (req.literal as Literal) : "string";
      const output = lang.render(req.which, {
        dialect: req.dialect,
        indent: req.indent ?? "",
        key: req.key ?? "",
        value: req.value ?? "",
        literal,
        old_key: req.old_key ?? "",
      });
      return { ok: true, output };
    }
    default:
      throw new LanguageError(`unknown op ${JSON.stringify((req as { op: string }).op)}`);
  }
}
