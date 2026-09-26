// fig — parsing, comment-preserving editing, and cross-format serialization
// of JSON, JSONC, JSON5, YAML, TOML, INI, dotenv, .properties, NestedText and
// fig (ZON and plist in an opt-in build, and any format registered at
// runtime), backed by the fig core compiled to WebAssembly.
//
// The module loads synchronously and imports nothing host-specific, so it works
// identically in Node and the browser.
export {
  Format,
  NodeKind,
  ExtKind,
  Status,
  FigError,
  WarningCode,
  WarningCause,
  type SerializeOptions,
  type Warning,
  type ParseDetail,
} from "./types.ts";
export { Document } from "./document.ts";
export { Editor } from "./editor.ts";
export { Embed, EmbedType, type Region, type Span } from "./embed.ts";
export { type Segment } from "./edit-ops.ts";
export { V, fromJS, toJS, stringify, diagnose, type Value, type JsValue, type JsInput } from "./value.ts";
export { version, versionString, capabilities, type Version, type Capabilities } from "./meta.ts";
export { init, isReady } from "./ffi.ts";
export { registerLanguage, formatByName } from "./language.ts";
export {
  handle,
  describe,
  LanguageError,
  type Language,
  type Dialect,
  type Syntax,
  type Comments,
  type CommentDelimiter,
  type SectionHeader,
  type ClosedContainers,
  type NativeKinds,
  type Renderer,
  type Literal,
  type RenderArgs,
  type PrintOptions,
  type NodeTable,
  type NodeRow,
  type RowKind,
  type RowExtKind,
  type RowSpan,
  type RegionRow,
  type MentionRow,
  type CommentRow,
  type DirectiveRow,
} from "./wire.ts";
export { serve, type HelperIo } from "./helper.ts";

import { Document } from "./document.ts";
import type { Format, SerializeOptions } from "./types.ts";
import type { JsValue } from "./value.ts";

/** Parse `input` in `format` directly to plain JavaScript values. Convenience
 *  over `Document.parse(...).toJS()` that releases the handle for you. The
 *  optional type parameter lets you assert the shape you expect —
 *  `parse<Config>(text, Format.Toml)` — with no runtime check. */
export function parse<T = JsValue>(input: string | Uint8Array, format: Format): T {
  const doc = Document.parse(input, format);
  try {
    return doc.toJS() as T;
  } finally {
    doc.dispose();
  }
}

/** Convert `input` from `from` to `to` in one call — the cross-format primitive
 *  (e.g. `convert(yamlText, Format.Yaml, Format.Json)`). Preserves comments where
 *  the target allows and collapses YAML's reference layer, exactly like
 *  {@link Document#serialize}; pass `{ lossless: true }` to round-trip values the
 *  target cannot natively represent. Releases the handle for you. */
export function convert(
  input: string | Uint8Array,
  from: Format,
  to: Format,
  options?: SerializeOptions,
): string {
  const doc = Document.parse(input, from);
  try {
    return doc.serialize(to, options);
  } finally {
    doc.dispose();
  }
}
