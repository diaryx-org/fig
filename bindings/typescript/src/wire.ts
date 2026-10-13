// The helper wire, in JavaScript: what a format written in JavaScript is
// (`Language`), and one request line to one response line (`handle`).
//
// The contract is fig's helper wire — JSON-RPC 2.0, one message per line,
// which a `fig` command line speaks to a helper process, specified in
// docs/helper-protocol.md. A `Language`
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

// ── the format ─────────────────────────────────────────────────────────────

declare const formatBrand: unique symbol;

/** A format: the handle of a registered dialect. Handles are assigned per
 *  process, in registration order, so persist a format's NAME and resolve
 *  it with `formatByName` — never the number. A `Format` comes only from
 *  `registerLanguage`, `formatByName`, `requireFormat`, `detectFormat` or a
 *  registered language's `format`. Declared here, beside the `Language`
 *  that carries one, so that this module still imports nothing. */
export type Format = number & { readonly [formatBrand]: true };

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
  line?: CommentDelimiter | null;
  trailing?: CommentDelimiter | null;
  /** A paired comment that may run over several lines (`/*`, `*\/`), which
   *  the owned-comment scan walks as a unit. */
  block?: CommentDelimiter | null;
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
  section_noun?: string | null;
  section_header?: SectionHeader | null;
  merge_key?: string | null;
  /** The bytes between two members of a flow container; absent is `,`. */
  flow_entry_sep?: string | null;
  /** The root is a flow mapping with no delimiters (a CSS `style` attribute). */
  flow_root?: boolean;
  /** A `[` does not open a flow sequence. */
  flow_maps_only?: boolean;
  /** Bytes besides whitespace and `seq_item_marker`'s that may stand before
   *  a node at the start of its line; absent is none. */
  line_prefix_marks?: string | null;
  /** The key/value separator of a flow object's entries where it differs
   *  from a block entry's (figl's `=` in `{a=1}`); absent is none. */
  flow_map_kv_sep?: string | null;
  /** The bytes between two members of a flow object written with
   *  `flow_map_kv_sep` (figl's ` `); absent is `flow_entry_sep` and a space. */
  flow_map_entry_sep?: string | null;
  /** The byte that, last on a line, carries it on to the next (92 for
   *  `.properties`' `\`), as its code, or 0 for none. */
  line_continuation?: number;
  /** The empty list `set` seeds to start one that is absent (`[]`); absent
   *  or null for a format with no spelling for one. */
  empty_seq_literal?: string | null;
}

/** Which kinds the format holds natively — what the `$fig` lossless
 *  envelope need not wrap. */
export interface NativeKinds {
  null?: boolean;
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
  splice?: "literal" | "escaped" | "raw";
  /** What `set` writes to a file that does not exist yet; `null` refuses
   *  creation. */
  empty_doc_seed?: string | null;
  /** This dialect's own syntax where it differs from the language's. */
  syntax?: Syntax | null;
  /** What the dialect's printer makes of a tree from another format, which
   *  fig's conversion warnings are computed from; absent leaves them to
   *  `caps` and `lossless`. */
  prints?: Prints | null;
}

/** What a dialect's printer writes of a tree read from another format. */
export interface Prints {
  /** The kinds written as themselves, named as `lossless` names them. A
   *  `null` not held is dropped; an extended scalar not held degrades. */
  holds: NativeKinds;
  /** Booleans and numbers written as themselves; `false` where every scalar
   *  is text. Absent is `true`. */
  typed_scalars?: boolean;
  /** A `null` at the root is an empty document, not a loss. */
  null_at_root?: boolean;
  /** The root must be a mapping or a sequence. */
  root_container?: boolean;
  /** What a char literal not held is written as; absent is "string". */
  char_as?: string;
  /** What a non-finite float not held is written as; absent is "string". */
  number_special_as?: string;
  /** When the printer writes comments; absent is "always". */
  comments?: "always" | "pretty" | "never";
}

/** One document of a multi-document stream: its byte range `[start, end)`
 *  of the stream's UTF-8 input and whether it opened with a document-start
 *  marker. */
export interface StreamRange {
  start: number;
  end: number;
  explicit: boolean;
}

/** The six fragment renderers the editor may ask a format for. */
export type Renderer = "value" | "entry" | "item" | "tail" | "key" | "key_name";

/** What fig's bare-literal rules made of a value's text — classified once
 *  by fig so that every format means the same thing by `42`. */
export type Literal = "null" | "bool" | "int" | "float" | "datetime" | "string";

/** The arguments to a renderer. `indent` is the target line's indentation;
 *  `key`, `value` and `old_key` are as written; `literal` is set for the
 *  value renderer alone. `parent_key` and `parent_tag`, for every
 *  renderer, are the container the fragment is written into — the mapping
 *  an entry joins, the sequence an item joins, the container holding a
 *  value replaced or a key renamed: the name of the key it stands under
 *  (empty at the root and for a container that is itself a sequence item)
 *  and its tag as a node row spells one (`!dependency`; empty for none). */
export interface RenderArgs {
  dialect: string;
  indent: string;
  key: string;
  value: string;
  literal: Literal;
  old_key: string;
  parent_key: string;
  parent_tag: string;
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
  /** Print a container root inline — fig's `{a=1}` / `[a, b]` — as a
   *  value spliced after `key = ` must be, since a block spelling re-reads
   *  there as a string. The editors set it for fig alone, so a language
   *  registered as fig answers it; any other ignores it. */
  flow: boolean;
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
  /** For a sequence whose items are joined by a separator of its own, not
   *  the dialect's `flow_entry_sep` nor one per line: the bytes written
   *  between two items on one line — `" | "`, `" "`, or `"\n\n"` for items
   *  a blank line apart. The sequence is then edited item by item with it:
   *  a delete takes one item and the separator beside it (this with its
   *  whitespace trimmed, or only whitespace where nothing is left), and an
   *  append splices after the last item, so a trailing separator stays
   *  trailing. Only a sequence may carry one; an empty one is none. */
  item_sep?: string;
  /** For a sequence: it has no delimiters, so its items run from its span's
   *  start to its end — a voice's bars after `Name:`. Makes the sequence a
   *  separated one, with the dialect's `flow_entry_sep` when `item_sep` is
   *  absent. */
  open?: boolean;
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
  /** What the parse found worth telling: warnings on input that parses. */
  diagnostics?: DiagnosticRow[];
}

/** A diagnostic a parse raises: an error or a warning at `span` (byte
 *  offsets), a sentence saying what is wrong and how to fix it, and a few
 *  words for a caret's label. */
export interface DiagnosticRow {
  severity: "error" | "warning";
  span: RowSpan;
  message: string;
  label?: string;
}

// ── the language ───────────────────────────────────────────────────────────

/** A format written in JavaScript: the wire's `description` — what the
 *  format declares — with the functions the format is. Hand one to
 *  {@link registerLanguage}, or {@link serve} it to a `fig` command line. */
export interface Language {
  /** Must differ from the name of every language already registered;
   *  `dialects[0].name` must equal it. */
  name: string;
  /** The format of the first dialect, once {@link registerLanguage} has
   *  registered this object — `yaml.format` after `registerLanguage(yaml)`
   *  — and `undefined` before. Written by registration, never read from
   *  the description; the other dialects are found by name
   *  ({@link formatByName}). */
  readonly format?: Format;
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
  /** Which of `render`'s six fragments this format spells itself. */
  renderers?: Renderer[];

  /** The input — text; the wire carries no bytes that are not — to its
   *  node table. Refuse it by throwing a {@link LanguageError}, whose
   *  `byteOffset` is where; any other throw is reported without one. */
  parse(dialect: string, input: string): NodeTable;
  /** Required where `caps.serialize` is set. A table whose root is a scalar
   *  is a value on its own: spell it as a document holding just that value
   *  where the format has one (plist's `<plist>` around it), and as the
   *  scalar alone where it has none (fig). `options.splice` asks for the
   *  bare value either way. */
  print?(dialect: string, table: NodeTable, options: PrintOptions): string;
  /** Answer one of the renderers `renderers` declares. */
  render?(which: Renderer, args: RenderArgs): string;
  /** Locate the documents of a multi-document stream (YAML's `---`/`...`):
   *  each one's BYTE range of the UTF-8 input, to be parsed on its own, and
   *  whether it opened with a document-start marker. An empty stream is one
   *  document. */
  split_stream?(dialect: string, input: string): StreamRange[];
}

/** How a language refuses its input: the message a caller sees, the byte
 *  offset it points at, if any, and, for a printer, which refusal it is —
 *  one of {@link Refusal}'s codes, so fig reports the failure it names. */
export class LanguageError extends Error {
  readonly byteOffset: number | undefined;
  readonly code: number | undefined;
  constructor(message: string, byteOffset?: number, code?: number) {
    super(message);
    this.name = "LanguageError";
    this.byteOffset = byteOffset;
    this.code = code;
  }
}

/** The reasons a printer gives for refusing a table, as the codes of a
 *  JSON-RPC error (docs/helper-protocol.md). `LanguageError`'s third
 *  argument. */
export const Refusal = {
  NullUnsupported: 1,
  NonStringKey: 2,
  UnrepresentableRoot: 3,
  UnsupportedValue: 4,
  InvalidKey: 5,
  NonFiniteNumber: 6,
  UnresolvedAlias: 7,
  NestingTooDeep: 8,
  UnexpectedNodeKind: 9,
} as const;

/** The code of an error a language raises with no more specific one — a
 *  parse it refuses, a renderer that cannot spell a fragment. JSON-RPC
 *  leaves -32000 to -32099 to the server. */
export const LANGUAGE_REFUSED = -32000;

// ── the wire ───────────────────────────────────────────────────────────────

/** A request's `params`, by method — what `handle` reads. */
type Params =
  | { method: "describe" }
  | { method: "parse"; params: { dialect: string; input: string } }
  | { method: "split_stream"; params: { dialect: string; input: string } }
  | { method: "print"; params: { dialect: string; table: NodeTable; options?: Partial<PrintOptions> } }
  | {
      method: "render";
      params: {
        which: Renderer;
        dialect: string;
        indent?: string;
        key?: string;
        value?: string;
        literal?: string;
        old_key?: string;
        parent_key?: string;
        parent_tag?: string;
      };
    };

/** A JSON-RPC error raised while reading a request, before any language
 *  code runs: its code is one JSON-RPC reserves. */
class ProtocolError extends Error {
  readonly code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
}

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
      prints: d.prints ?? null,
    })),
    samples: lang.samples,
    renderers: lang.renderers ?? [],
    split_stream: typeof lang.split_stream === "function",
  };
}

const RENDERERS: readonly Renderer[] = ["value", "entry", "item", "tail", "key", "key_name"];
const LITERALS: readonly Literal[] = ["null", "bool", "int", "float", "datetime", "string"];

/** One request line to one response line — the helper wire, JSON-RPC
 *  2.0, in JavaScript. What the wasm module calls for a registered
 *  language, and what {@link serve} runs over stdin and stdout. Never
 *  throws: a refusal, a malformed request and a bug in the language alike
 *  come back as an `error` response carrying the request's id. */
export function handle(lang: Language, requestLine: string): string {
  let id: number | string | null = null;
  try {
    let req: unknown;
    try {
      req = JSON.parse(requestLine);
    } catch (err) {
      throw new ProtocolError(-32700, `request is not JSON: ${err instanceof Error ? err.message : String(err)}`);
    }
    if (typeof req !== "object" || req === null || Array.isArray(req)) throw new ProtocolError(-32600, "a request is one JSON object");
    const r = req as { jsonrpc?: unknown; id?: unknown; method?: unknown; params?: unknown };
    if (typeof r.id === "number" || typeof r.id === "string") id = r.id;
    if (r.jsonrpc !== "2.0") throw new ProtocolError(-32600, 'a request carries "jsonrpc":"2.0"');
    if (typeof r.method !== "string") throw new ProtocolError(-32600, "a request names its method");
    if (r.params !== undefined && (typeof r.params !== "object" || r.params === null || Array.isArray(r.params))) {
      throw new ProtocolError(-32602, "params is an object");
    }
    const result = answer(lang, { method: r.method, params: r.params ?? {} } as Params);
    return JSON.stringify({ jsonrpc: "2.0", id, result });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    let code = LANGUAGE_REFUSED;
    let data: Record<string, unknown> | undefined;
    if (err instanceof ProtocolError) code = err.code;
    else if (err instanceof LanguageError) {
      if (err.code !== undefined) code = err.code;
      if (err.byteOffset !== undefined) data = { byte_offset: err.byteOffset };
    }
    const error = data === undefined ? { code, message } : { code, message, data };
    return JSON.stringify({ jsonrpc: "2.0", id, error });
  }
}

function invalidParams(message: string): never {
  throw new ProtocolError(-32602, message);
}

function answer(lang: Language, req: Params): unknown {
  switch (req.method) {
    case "describe":
      return describe(lang);
    case "parse": {
      const p = req.params;
      if (typeof p.dialect !== "string" || typeof p.input !== "string") invalidParams("parse: dialect and input are strings");
      const table = lang.parse(p.dialect, p.input);
      const out: Record<string, unknown> = { rows: table.rows, regions: table.regions ?? [], mentions: table.mentions ?? [], comments: table.comments ?? [] };
      if (table.directives && table.directives.length > 0) out.directives = table.directives;
      return out;
    }
    case "print": {
      const p = req.params;
      if (!lang.print) throw new ProtocolError(-32601, `${lang.name} does not serialize`);
      if (typeof p.dialect !== "string" || typeof p.table !== "object" || p.table === null || !Array.isArray(p.table.rows)) {
        invalidParams("print: dialect is a string and table a node table");
      }
      const o = p.options ?? {};
      const options: PrintOptions = {
        pretty: o.pretty ?? true,
        strip_comments: o.strip_comments ?? false,
        indent: o.indent ?? 2,
        width: o.width ?? 80,
        splice: o.splice ?? false,
        flow: o.flow ?? false,
      };
      return lang.print(p.dialect, p.table, options);
    }
    case "split_stream": {
      const p = req.params;
      if (!lang.split_stream) throw new ProtocolError(-32601, `${lang.name} has no multi-document streams`);
      if (typeof p.dialect !== "string" || typeof p.input !== "string") invalidParams("split_stream: dialect and input are strings");
      return lang.split_stream(p.dialect, p.input);
    }
    case "render": {
      const p = req.params;
      if (!lang.render) throw new ProtocolError(-32601, `${lang.name} declares no renderers`);
      if (!RENDERERS.includes(p.which)) invalidParams(`render: unknown renderer ${JSON.stringify(p.which)}`);
      if (typeof p.dialect !== "string") invalidParams("render: dialect is a string");
      // Absent or unknown: a string, which is what a renderer does with any
      // text it cannot type.
      const literal = LITERALS.includes(p.literal as Literal) ? (p.literal as Literal) : "string";
      return lang.render(p.which, {
        dialect: p.dialect,
        indent: p.indent ?? "",
        key: p.key ?? "",
        value: p.value ?? "",
        literal,
        old_key: p.old_key ?? "",
        parent_key: p.parent_key ?? "",
        parent_tag: p.parent_tag ?? "",
      });
    }
    default:
      throw new ProtocolError(-32601, `unknown method ${JSON.stringify((req as { method: string }).method)}`);
  }
}
