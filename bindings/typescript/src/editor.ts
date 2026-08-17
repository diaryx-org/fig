// Comment-preserving, in-place editing of a whole JSON/JSONC/JSON5/YAML/TOML
// document.
//
// Unlike `serialize`, which re-renders a whole value, `Editor` splices only the
// bytes of the node you change — comments, key order, blank lines, and quoting
// everywhere else stay byte-identical. Inserted values are rendered by fig's
// serializer (see `Editable`) and re-framed at the splice site. Release with
// `dispose` (or a `using` declaration).
import { check, FigError, Format, Status } from "./types.ts";
import { encodeKeyList, encodePath, fig, Frame, handleRegistry, readOutSlice } from "./ffi.ts";
import { Editable, type EditFns, type Segment } from "./edit-ops.ts";

const encoder = new TextEncoder();

// Frees the handle of an Editor dropped without dispose() (leak backstop only).
const REGISTRY = handleRegistry((handle) => fig.fig_editor_destroy(handle));

// Editor edits are rendered in the document's own format before splicing (a
// `Value` string becomes `"x"` for TOML/JSON but a bare `x` for YAML); the Zig
// editor then re-frames that text into the document's actual context.
//
// Each entry is a thunk rather than a direct `fig.fig_editor_*` reference: a
// direct reference would read the property off the lazy `fig` proxy at module
// load, forcing wasm instantiation the moment the package is imported — which
// throws on a browser main thread. Deferring the read to call time keeps import
// side-effect-free (see ffi.ts `init`).
const EDITOR_FNS: EditFns = {
  replaceVal: (...a) => fig.fig_editor_replace_val(...a),
  replaceKey: (...a) => fig.fig_editor_replace_key(...a),
  set: (...a) => fig.fig_editor_set(...a),
  insertKey: (...a) => fig.fig_editor_insert_key(...a),
  deleteKey: (...a) => fig.fig_editor_delete_key(...a),
  appendSeq: (...a) => fig.fig_editor_append_seq(...a),
  prependSeq: (...a) => fig.fig_editor_prepend_seq(...a),
  removeSeqItem: (...a) => fig.fig_editor_remove_seq_item(...a),
  moveKey: (...a) => fig.fig_editor_move_key(...a),
  reorderKeys: (...a) => fig.fig_editor_reorder_keys(...a),
  moveItem: (...a) => fig.fig_editor_move_item(...a),
  reorderItems: (...a) => fig.fig_editor_reorder_items(...a),
  setSequence: (...a) => fig.fig_editor_set_sequence(...a),
  addLeadingComment: (...a) => fig.fig_editor_add_leading_comment(...a),
  setTrailingComment: (...a) => fig.fig_editor_set_trailing_comment(...a),
  deleteLeadingComments: (...a) => fig.fig_editor_delete_leading_comments(...a),
  deleteTrailingComment: (...a) => fig.fig_editor_delete_trailing_comment(...a),
  getLeadingComment: (...a) => fig.fig_editor_get_leading_comment(...a),
  getTrailingComment: (...a) => fig.fig_editor_get_trailing_comment(...a),
};

export class Editor extends Editable {
  private constructor(handle: number, format: Format) {
    super(handle, EDITOR_FNS, format);
    REGISTRY?.register(this, handle, this);
  }

  /** Open an editor over a copy of `input` in `format`
   *  (Json/Jsonc/Json5/Yaml/Toml). Empty input is a valid empty document. */
  static open(input: string | Uint8Array, format: Format): Editor {
    const bytes = typeof input === "string" ? encoder.encode(input) : input;
    const frame = new Frame();
    const out = frame.alloc(4);
    try {
      const ptr = frame.bytes(bytes);
      check(fig.fig_editor_create(ptr, bytes.length, format, out), "fig_editor_create");
      const handle = new DataView(fig.memory.buffer).getUint32(out, true);
      if (handle === 0) throw new FigError(Status.InternalError, "fig_editor_create");
      return new Editor(handle, format);
    } finally {
      frame.dispose();
    }
  }

  /** The editor's current source text, reflecting all edits so far. */
  source(): string {
    const frame = new Frame();
    try {
      const scratch = frame.alloc(8);
      check(fig.fig_editor_source(this.live(), scratch, scratch + 4), "fig_editor_source");
      return readOutSlice(scratch);
    } finally {
      frame.dispose();
    }
  }

  // ── whole containers ────────────────────────────────────────────────────
  //
  // The inherited edits address a container the same way they address a
  // scalar: by the one range of source it occupies. A TOML `[header]` table
  // occupies no such range — its body is the lines after the header, and an
  // `[a.b]` header further down the file extends it — and neither does an INI
  // `[section]` or a `fig` block container. At a path naming one, `delete`,
  // `replaceValue`, `moveKey` and `reorderKeys` all fail with
  // `Status.InvalidArgument` rather than rewrite the header and leave its
  // entries behind. These six are the route for those shapes.
  //
  // Support varies by format, and an unsupported op fails with
  // `Status.UnsupportedFormat`: Toml has all six, Ini and Fig the
  // delete/move/reorder three, every other format none. That is not a gap —
  // Yaml, Json and the rest nest a container in one contiguous region, so the
  // inherited edits already handle it.
  //
  // These live on `Editor` rather than on `Editable` because the C ABI has no
  // `fig_embed_*` twins: an embedded region is opened by archetype, and no
  // archetype hosts a scattered-container format today.

  /** Delete the whole container at `path` — every scattered region of its
   *  subtree, leaving interleaved foreign content in place. */
  deleteContainer(path: readonly Segment[]): void {
    const frame = new Frame();
    try {
      const p = encodePath(frame, path);
      check(fig.fig_editor_delete_container(this.live(), p.ptr, p.len), "deleteContainer");
    } finally {
      frame.dispose();
    }
  }

  /** Create a container at `path` whose entries are `body` (verbatim lines in
   *  the document's format, e.g. `ip = "10.0.0.1"\n`), spliced past the
   *  parent's whole subtree so no existing key is reparented. A body that
   *  doesn't parse rolls the document back. */
  insertContainer(path: readonly Segment[], body: string): void {
    const frame = new Frame();
    try {
      const p = encodePath(frame, path);
      const b = frame.str(body);
      check(fig.fig_editor_insert_container(this.live(), p.ptr, p.len, b.ptr, b.len), "insertContainer");
    } finally {
      frame.dispose();
    }
  }

  /** Rename the container at `path` to `newLeaf`, rewriting every line that
   *  names it: renaming `a` rewrites `[a]`, `[a.b]` and `[[a.c]]` alike. */
  renameContainer(path: readonly Segment[], newLeaf: string): void {
    const frame = new Frame();
    try {
      const p = encodePath(frame, path);
      const l = frame.str(newLeaf);
      check(fig.fig_editor_rename_container(this.live(), p.ptr, p.len, l.ptr, l.len), "renameContainer");
    } finally {
      frame.dispose();
    }
  }

  /** Move the container at `src` before the one at `dest`, re-emitting its
   *  scattered fragments contiguously; interleaved foreign containers stay
   *  put. `dest` of `null` moves it to the end of the document. */
  moveContainer(src: readonly Segment[], dest: readonly Segment[] | null): void {
    // The ABI carries "to EOF" as a NULL destination pointer, and an empty path
    // encodes to pointer 0 — so an empty array would silently become EOF here
    // rather than naming the root the way it does everywhere else. The root is
    // not a container and cannot be a destination, so reject it outright
    // instead of quietly meaning something else.
    if (dest !== null && dest.length === 0) {
      throw new FigError(Status.InvalidArgument, "moveContainer: the root is not a valid destination — pass null to move to the end of the document");
    }
    const frame = new Frame();
    try {
      const s = encodePath(frame, src);
      const d = dest === null ? { ptr: 0, len: 0 } : encodePath(frame, dest);
      check(fig.fig_editor_move_container(this.live(), s.ptr, s.len, d.ptr, d.len), "moveContainer");
    } finally {
      frame.dispose();
    }
  }

  /** Reorder top-level containers so those named in `order` come first, in
   *  that order, each re-emitted contiguously at the position the earliest of
   *  them currently occupies. Containers not named keep their places. */
  reorderContainers(order: readonly string[]): void {
    const frame = new Frame();
    try {
      const o = encodeKeyList(frame, order);
      check(fig.fig_editor_reorder_containers(this.live(), o.ptr, o.len), "reorderContainers");
    } finally {
      frame.dispose();
    }
  }

  /** Append an element with body `body` to the container sequence at `path` —
   *  TOML's `[[header]]` array-of-tables append. */
  appendContainerToSeq(path: readonly Segment[], body: string): void {
    const frame = new Frame();
    try {
      const p = encodePath(frame, path);
      const b = frame.str(body);
      check(fig.fig_editor_append_container_to_seq(this.live(), p.ptr, p.len, b.ptr, b.len), "appendContainerToSeq");
    } finally {
      frame.dispose();
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    REGISTRY?.unregister(this);
    fig.fig_editor_destroy(this.handle);
  }
}
