// Comment-preserving editing of a config embedded in a host file — markdown
// frontmatter (`---`, `---<lang>`, `;;;`, `+++`), a fenced code block, a
// trailing endmatter block, or an HTML `<script>` / `<pre><code>` island,
// holding JSON, YAML, TOML or fig.
//
// `Embed.open` locates the region, edits its content in the region's inner
// format (fixed by the archetype), and `render` re-assembles the host file with
// the fences and surrounding text byte-identical. The edit methods are
// inherited from `Editable`. The parse-free locators are statics beside it —
// `Embed.extract` (the byte spans), `Embed.split` (the text) and
// `Embed.detect` (which archetype) — and so is `Embed.retype`. Release with
// `dispose`.
import { check, FigError, Format, Status, type ParseDetail } from "./types.ts";
import { fig, Frame, handleRegistry, probeParse, readOutSlice, readU32, writeU32 } from "./ffi.ts";
import { Editable, type EditFns } from "./edit-ops.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** An embed archetype: where the block sits in the host, how it is fenced, and
 *  which format its content is written in. Each value is the archetype's name
 *  as the `fig` command line spells it (`fig get --embed fenced-fig …`), so a
 *  stored `EmbedType` is readable and means the same thing to both.
 *
 *  | Family | Members | Fence |
 *  | ------ | ------- | ----- |
 *  | Markdown frontmatter | `Frontmatter` (YAML), `MdJson`, `MdToml`, `MdFig` | `---`, `---json`, … |
 *  | Fenced code block | `FencedYaml`, `FencedJson`, `FencedToml`, `FencedFig` | ```` ```<lang> ```` |
 *  | HTML data island | `HtmlScriptYaml`, …`Json`, …`Toml`, …`Fig` | `<script type="application/<lang>">` |
 *  | HTML visible code | `HtmlCodeYaml`, …`Json`, …`Toml`, …`Fig` | `<pre><code class="language-<lang>">`, entity-encoded |
 *  | Presets | `Semicolons` (JSON), `Plus` (TOML), `Endmatter` (YAML) | `;;;`, `+++`, a trailing ```` ```endmatter ```` |
 *
 *  A closed set: the binding names every archetype the core can locate, and
 *  {@link Embed.detect} answers `null` for a pair it has no name for. An
 *  `as const` object like the numeric enums (see the ENUMS note atop
 *  types.ts). */
export const EmbedType = {
  Frontmatter: "frontmatter",
  MdJson: "md-json",
  MdToml: "md-toml",
  MdFig: "md-fig",
  FencedYaml: "fenced-yaml",
  FencedJson: "fenced-json",
  FencedToml: "fenced-toml",
  FencedFig: "fenced-fig",
  HtmlScriptYaml: "html-script-yaml",
  HtmlScriptJson: "html-script-json",
  HtmlScriptToml: "html-script-toml",
  HtmlScriptFig: "html-script-fig",
  HtmlCodeYaml: "html-code-yaml",
  HtmlCodeJson: "html-code-json",
  HtmlCodeToml: "html-code-toml",
  HtmlCodeFig: "html-code-fig",
  Semicolons: "semicolons",
  Plus: "plus",
  Endmatter: "endmatter",
} as const;
export type EmbedType = (typeof EmbedType)[keyof typeof EmbedType];

// The container half of the C ABI's embed selector (`FigEmbedContainer`),
// which takes a (container, format) pair where this binding takes one name.
// Internal: `EmbedType` is the whole public vocabulary.
const Container = {
  MdFrontmatter: 0, //  ---<lang> (bare --- is YAML)
  Fenced: 1, //         ```<lang>
  HtmlScript: 2, //     <script type="application/<lang>">
  HtmlCode: 3, //       <pre><code class="language-<lang>"> (entity-encoded)
  SemicolonsJson: 4, // ;;; preset, JSON
  PlusToml: 5, //       +++ preset, TOML
  EndmatterYaml: 6, //  ```endmatter preset, YAML
} as const;

/** Each archetype's (container, inner format) pair: the C ABI's selector, and
 *  the format the embed's content is edited in. */
const PARTS: Record<EmbedType, readonly [number, Format]> = {
  [EmbedType.Frontmatter]: [Container.MdFrontmatter, Format.Yaml],
  [EmbedType.MdJson]: [Container.MdFrontmatter, Format.Json],
  [EmbedType.MdToml]: [Container.MdFrontmatter, Format.Toml],
  [EmbedType.MdFig]: [Container.MdFrontmatter, Format.Fig],
  [EmbedType.FencedYaml]: [Container.Fenced, Format.Yaml],
  [EmbedType.FencedJson]: [Container.Fenced, Format.Json],
  [EmbedType.FencedToml]: [Container.Fenced, Format.Toml],
  [EmbedType.FencedFig]: [Container.Fenced, Format.Fig],
  [EmbedType.HtmlScriptYaml]: [Container.HtmlScript, Format.Yaml],
  [EmbedType.HtmlScriptJson]: [Container.HtmlScript, Format.Json],
  [EmbedType.HtmlScriptToml]: [Container.HtmlScript, Format.Toml],
  [EmbedType.HtmlScriptFig]: [Container.HtmlScript, Format.Fig],
  [EmbedType.HtmlCodeYaml]: [Container.HtmlCode, Format.Yaml],
  [EmbedType.HtmlCodeJson]: [Container.HtmlCode, Format.Json],
  [EmbedType.HtmlCodeToml]: [Container.HtmlCode, Format.Toml],
  [EmbedType.HtmlCodeFig]: [Container.HtmlCode, Format.Fig],
  [EmbedType.Semicolons]: [Container.SemicolonsJson, Format.Json],
  [EmbedType.Plus]: [Container.PlusToml, Format.Toml],
  [EmbedType.Endmatter]: [Container.EndmatterYaml, Format.Yaml],
};

/** The {@link EmbedType} for a pair the core reports, or `null` for one this
 *  binding has no name for. A preset container pins its own format, so the
 *  format half is not compared for one. */
function embedTypeOf(container: number, format: number): EmbedType | null {
  for (const [kind, [c, f]] of Object.entries(PARTS) as Array<[EmbedType, readonly [number, Format]]>) {
    if (c === container && (f === format || c >= Container.SemicolonsJson)) return kind;
  }
  return null;
}

/** `PARTS[kind]`, refusing a string that is not an archetype — the type says
 *  `EmbedType`, but a value read from storage or a plain-JS caller may not be
 *  one, and the C ABI should not be handed `undefined`. */
function partsOf(kind: EmbedType): readonly [number, Format] {
  // `hasOwn`, not a lookup: `PARTS` is a plain object, and `"constructor"` or
  // `"toString"` would find what it inherits from `Object.prototype`.
  if (!Object.hasOwn(PARTS, kind)) throw new FigError(Status.InvalidArgument, "embed", { message: `unknown embed archetype ${JSON.stringify(kind)}` });
  return PARTS[kind];
}

// Frees the handle of an Embed dropped without dispose() (leak backstop only).
const REGISTRY = handleRegistry((handle) => fig.fig_embed_destroy(handle));

// Thunks, not direct `fig.fig_embed_*` references: a direct reference would read
// off the lazy `fig` proxy at module load and force wasm instantiation on
// import (which throws on a browser main thread). See editor.ts / ffi.ts `init`.
const EMBED_FNS: EditFns = {
  replaceVal: (...a) => fig.fig_embed_replace_val(...a),
  replaceNamedKey: (...a) => fig.fig_embed_replace_named_key(...a),
  set: (...a) => fig.fig_embed_set(...a),
  insertNamedKey: (...a) => fig.fig_embed_insert_named_key(...a),
  deleteKey: (...a) => fig.fig_embed_delete_key(...a),
  appendSeq: (...a) => fig.fig_embed_append_seq(...a),
  prependSeq: (...a) => fig.fig_embed_prepend_seq(...a),
  removeSeqItem: (...a) => fig.fig_embed_remove_seq_item(...a),
  moveKey: (...a) => fig.fig_embed_move_key(...a),
  reorderKeys: (...a) => fig.fig_embed_reorder_keys(...a),
  moveItem: (...a) => fig.fig_embed_move_item(...a),
  reorderItems: (...a) => fig.fig_embed_reorder_items(...a),
  setSequence: (...a) => fig.fig_embed_set_sequence(...a),
  addLeadingComment: (...a) => fig.fig_embed_add_leading_comment(...a),
  setTrailingComment: (...a) => fig.fig_embed_set_trailing_comment(...a),
  deleteLeadingComments: (...a) => fig.fig_embed_delete_leading_comments(...a),
  deleteTrailingComment: (...a) => fig.fig_embed_delete_trailing_comment(...a),
  getLeadingComment: (...a) => fig.fig_embed_get_leading_comment(...a),
  getTrailingComment: (...a) => fig.fig_embed_get_trailing_comment(...a),
  addDanglingComment: (...a) => fig.fig_embed_add_dangling_comment(...a),
  deleteDanglingComments: (...a) => fig.fig_embed_delete_dangling_comments(...a),
  getDanglingComment: (...a) => fig.fig_embed_get_dangling_comment(...a),
  commentOut: (...a) => fig.fig_embed_comment_out(...a),
  uncommentLeading: (...a) => fig.fig_embed_uncomment_leading(...a),
  uncommentDangling: (...a) => fig.fig_embed_uncomment_dangling(...a),
};

/** A half-open `[start, end)` byte span within the host file. */
export interface Span {
  start: number;
  end: number;
}

/** The byte spans of a located embedded region.
 *
 *  `bodyBefore` and `bodyAfter` are the host text on either side of the block.
 *  With the three region spans they tile the source exactly —
 *  `bodyBefore ++ openFence ++ content ++ closeFence ++ bodyAfter === source`,
 *  a leading UTF-8 BOM heading `bodyBefore` — so a caller can rebuild the host
 *  without losing a byte. */
export interface Region {
  openFence: Span;
  content: Span;
  closeFence: Span;
  bodyBefore: Span;
  bodyAfter: Span;
}

export class Embed extends Editable {
  private constructor(handle: number, kind: EmbedType) {
    super(handle, EMBED_FNS, partsOf(kind)[1]);
    REGISTRY?.register(this, handle, this);
  }

  private static openWith(
    host: string | Uint8Array,
    kind: EmbedType,
    fn: (input: number, inputLen: number, container: number, format: number, out: number) => number,
    name: string,
  ): Embed {
    const bytes = typeof host === "string" ? encoder.encode(host) : host;
    const [container, format] = partsOf(kind);
    const frame = new Frame();
    const out = frame.alloc(4);
    try {
      const ptr = frame.bytes(bytes);
      const status = fn(ptr, bytes.length, container, format, out);
      if (status === Status.ParseError) throw new FigError(status, name, Embed.parseDetail(bytes, kind));
      check(status, name);
      const handle = new DataView(fig.memory.buffer).getUint32(out, true);
      if (handle === 0) throw new FigError(Status.InternalError, name);
      return new Embed(handle, kind);
    } finally {
      frame.dispose();
    }
  }

  /** Why opening `kind` in `host` failed with `ParseError`, in the core's
   *  words where it has some (the open calls report a bare status; see
   *  `probeParse`). A block that never closes is said so. Otherwise its
   *  content is parsed again through `fig_parse_ex`, and the offset it
   *  reports is moved from the content into the host. The `HtmlCode*`
   *  archetypes are parsed after entity-decoding, which the binding does not
   *  repeat, so for those the status stands alone. */
  private static parseDetail(host: Uint8Array, kind: EmbedType): ParseDetail | undefined {
    let region: Region;
    try {
      region = Embed.extract(host, kind);
    } catch (err) {
      if (err instanceof FigError && err.status === Status.ParseError) {
        return { message: `the ${kind} block's opening delimiter has no matching close` };
      }
      return undefined;
    }
    if (partsOf(kind)[0] === Container.HtmlCode) return undefined;
    const detail = probeParse(host.subarray(region.content.start, region.content.end), partsOf(kind)[1]);
    if (detail === null) return undefined;
    // `line`/`column` would be content-relative; the host-relative offset is
    // the location that means something to the caller.
    return {
      message: detail.message,
      byteOffset: detail.byteOffset === undefined ? undefined : region.content.start + detail.byteOffset,
    };
  }

  /** Open the embed of `kind` in `host`. Throws {@link FigError} `NotFound` if
   *  no such region exists, and `ParseError` — with the core's message where
   *  it has one — if the block never closes or its content does not parse. */
  static open(host: string | Uint8Array, kind: EmbedType): Embed {
    return Embed.openWith(host, kind, fig.fig_embed_open, "fig_embed_open");
  }

  /** Open the embed of `kind` in `host`, creating an empty region when none
   *  exists (frontmatter at the top, endmatter at the bottom) instead of throwing
   *  `NotFound` — so a subsequent {@link set}/{@link insertValue} lands the first
   *  entry. An existing region is opened unchanged; a malformed one still throws.
   *  A block that goes at the top throws `UnsupportedOperation` when `host`
   *  already opens with frontmatter of another archetype, rather than pushing
   *  it off the first line. */
  static openOrInit(host: string | Uint8Array, kind: EmbedType): Embed {
    return Embed.openWith(host, kind, fig.fig_embed_open_or_init, "fig_embed_open_or_init");
  }

  /** Re-house `host`'s embedded region under a different archetype's fences:
   *  keep every host byte outside the block, and wrap `content` — the already
   *  re-serialized inner document, in `to`'s inner format — in `to`'s
   *  convention. The splice half of "convert this file's embed style"; the
   *  caller does the format conversion, fig does the fences and the placement.
   *
   *  The block MOVES only when `to` puts it at the other end of the file
   *  (frontmatter <-> endmatter); otherwise it is re-housed exactly where it
   *  sat, so retyping to the same archetype is a byte-identical rebuild. The
   *  host text on both sides survives in file order either way, and a UTF-8 BOM
   *  is re-emitted at offset 0 rather than travelling with the prose it
   *  precedes.
   *
   *  Throws {@link FigError} `UnsupportedOperation` when `from` is a
   *  mid-document archetype (`HtmlScript*`/`HtmlCode*`) and `to` sits at an edge
   *  of the file: hoisting a `---` fence above `<html>` is neither valid
   *  markdown nor valid HTML, and leaving the block where it is does not make it
   *  frontmatter. Mid-document to mid-document is fine, and splices in place.
   *  `NotFound` when `host` has no region of `from`; `ParseError` when it opens
   *  one and never closes it. */
  static retype(
    host: string | Uint8Array,
    from: EmbedType,
    to: EmbedType,
    content: string | Uint8Array,
  ): string {
    const hostBytes = typeof host === "string" ? encoder.encode(host) : host;
    const contentBytes = typeof content === "string" ? encoder.encode(content) : content;
    const [fromContainer, fromFormat] = partsOf(from);
    const [toContainer, toFormat] = partsOf(to);
    const frame = new Frame();
    try {
      const h = frame.bytes(hostBytes);
      const c = frame.bytes(contentBytes);
      // An 8-byte scratch holding the (ptr, len) out-param pair.
      const out = frame.alloc(8);
      check(
        fig.fig_embed_retype(
          h,
          hostBytes.length,
          fromContainer,
          fromFormat,
          toContainer,
          toFormat,
          c,
          contentBytes.length,
          out,
          out + 4,
        ),
        "fig_embed_retype",
      );
      // Unlike every other fig call, the result buffer is OURS: fig allocated it
      // and holds no handle to free it later. Copy it out, then hand it back
      // with the exact length — in a `finally`, so a decode failure still frees.
      const ptr = readU32(out);
      const len = readU32(out + 4);
      try {
        return readOutSlice(out);
      } finally {
        fig.fig_free(ptr, len);
      }
    } finally {
      frame.dispose();
    }
  }

  /** Locate an embedded region and report its fence/content spans without
   *  parsing the content. Throws {@link FigError} `NotFound` if absent. */
  static extract(input: string | Uint8Array, kind: EmbedType): Region {
    const bytes = typeof input === "string" ? encoder.encode(input) : input;
    const frame = new Frame();
    try {
      const ptr = frame.bytes(bytes);
      // FigRegion (wasm32): u32 size + 6 × FigSpan(u32 start, u32 end) = 52 bytes.
      // The caller must set `size` before the call so the size-gated library
      // fills the fields this layout declares — `bodyBefore`/`bodyAfter` are the
      // trailing pair added in core 2.7.0, which is exactly what `size` is for.
      const REGION_SIZE = 52;
      const region = frame.alloc(REGION_SIZE);
      writeU32(region, REGION_SIZE);
      const [container, format] = partsOf(kind);
      check(fig.fig_embed_extract(ptr, bytes.length, container, format, region), "fig_embed_extract");
      // Spans start after the 4-byte `size` field: offsets 4, 12, 20, 28, 36,
      // 44. The span at 28 is the C struct's one-sided `body`, which the two
      // sides after it supersede; it is not surfaced.
      const span = (off: number): Span => ({ start: readU32(region + off), end: readU32(region + off + 4) });
      return {
        openFence: span(4),
        content: span(12),
        closeFence: span(20),
        bodyBefore: span(36),
        bodyAfter: span(44),
      };
    } finally {
      frame.dispose();
    }
  }

  /** Split an embedded region of `kind` from its host without parsing — the
   *  read-only `[content, body]` twin of opening an {@link Embed}. Returns
   *  `null` when `host` has no such region (or its opening fence has no
   *  close). The first item is the text between the fences (no fences); the
   *  second is the host with the whole block cut out — `bodyBefore` then
   *  `bodyAfter` — which is the prose after frontmatter, the prose before
   *  endmatter, and both sides of a mid-document island. Slicing is done on
   *  UTF-8 bytes, so multi-byte content is handled correctly. */
  static split(host: string | Uint8Array, kind: EmbedType): [string, string] | null {
    const bytes = typeof host === "string" ? encoder.encode(host) : host;
    let region: Region;
    try {
      region = Embed.extract(bytes, kind);
    } catch (err) {
      // NotFound / unterminated fence; anything else is not "no region".
      if (err instanceof FigError && (err.status === Status.NotFound || err.status === Status.ParseError)) return null;
      throw err;
    }
    const text = (s: Span) => decoder.decode(bytes.subarray(s.start, s.end));
    return [text(region.content), text(region.bodyBefore) + text(region.bodyAfter)];
  }

  /** Best-effort sniff of which embed archetype `host` uses: try each known
   *  archetype's OPEN delimiter and return the first that matches, or `null`
   *  when `host` opens none of them. Only the open delimiter is checked — an
   *  unterminated block is still *recognized* as its archetype, so a follow-up
   *  {@link Embed.extract}/{@link Embed.open} surfaces the real error instead
   *  of a misleading "nothing found". */
  static detect(host: string | Uint8Array): EmbedType | null {
    const bytes = typeof host === "string" ? encoder.encode(host) : host;
    const frame = new Frame();
    try {
      const ptr = frame.bytes(bytes);
      // Two 4-byte out slots: the container, then the inner format.
      const out = frame.alloc(8);
      const status = fig.fig_embed_detect(ptr, bytes.length, out, out + 4);
      if (status === Status.NotFound) return null;
      check(status, "fig_embed_detect");
      return embedTypeOf(readU32(out), readU32(out + 4));
    } finally {
      frame.dispose();
    }
  }

  /** Replace the host body — the prose the config is embedded in — with `body`,
   *  keeping the fences and the current (possibly edited) content byte-identical.
   *  The body is the suffix after the close fence (frontmatter) or the prefix
   *  before the open fence (endmatter); only that side is swapped. `body` is
   *  taken verbatim (not parsed); an empty string clears it. Composes with the
   *  value edits — change keys, replace the body, then `render` once. */
  replaceBody(body: string): void {
    const frame = new Frame();
    try {
      const b = frame.str(body);
      check(fig.fig_embed_replace_body(this.live(), b.ptr, b.len), "replaceBody");
    } finally {
      frame.dispose();
    }
  }

  /** Render the full host file with the edited embed spliced back between the
   *  original fences. */
  render(): string {
    const frame = new Frame();
    try {
      const scratch = frame.alloc(8);
      check(fig.fig_embed_render(this.live(), scratch, scratch + 4), "fig_embed_render");
      return readOutSlice(scratch);
    } finally {
      frame.dispose();
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    REGISTRY?.unregister(this);
    fig.fig_embed_destroy(this.handle);
  }
}
