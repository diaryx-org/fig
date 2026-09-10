#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// ============================================================================
// Version
//
// FIG_VERSION_* are the version of THIS header (compile-time). fig_version()
// returns the version of the linked library at runtime, packed identically; a
// host can compare the two to detect a header/library skew. The components are
// also exposed as a string by fig_version_string().
// ============================================================================
#define FIG_VERSION_MAJOR 3
#define FIG_VERSION_MINOR 0
#define FIG_VERSION_PATCH 0
#define FIG_VERSION_NUM (((uint32_t)FIG_VERSION_MAJOR << 16) | \
                         ((uint32_t)FIG_VERSION_MINOR << 8)  | \
                         (uint32_t)FIG_VERSION_PATCH)

// Binary C ABI contract version — a monotonic counter, distinct from the
// marketing FIG_VERSION_* above. It identifies the *shape* of this header's ABI
// (symbols, struct layouts, enum values) the way an ELF SONAME does, and is
// bumped ONLY when that shape changes incompatibly. fig's forward-compat policy
// (size-gated structs, decode-unknown enums, add-never-remove functions) is
// designed so additions are non-breaking, so this stays put across feature
// releases and moves only on a true break. A host that dynamically loads libfig
// can compare fig_abi_version() against the FIG_ABI_VERSION it compiled with: a
// runtime value LOWER than the compile-time one means missing ABI it may rely
// on; a HIGHER value is an incompatible ABI it was not built for. (`zig build
// abi-check` pins this macro to the library; `zig build semver-check` requires
// it to increment whenever the ABI diff against the last release is breaking.)
//
// History: 1 — core 2.0 through 2.9. 2 — core 3.0: FigEmbedType folded into
// (FigEmbedContainer, FigFormat) pairs on every fig_embed_* selector, and
// FIG_FORMAT_XML retired.
#define FIG_ABI_VERSION 2

// Linked-library version, packed as (major << 16) | (minor << 8) | patch.
uint32_t fig_version(void);
// Linked-library version as a null-terminated "major.minor.patch" string. Static
// storage owned by the library; do not free.
const char *fig_version_string(void);
// Binary C ABI contract version of the linked library (see FIG_ABI_VERSION).
uint32_t fig_abi_version(void);

// ============================================================================
// Threading and memory ownership
//
// Threading: fig keeps no shared mutable global state, so calls that touch
// DIFFERENT handles (or no handle, e.g. fig_parse, fig_version,
// fig_format_capabilities) may run concurrently on different threads. A SINGLE
// handle — FigDocument, FigEditor, FigEmbed, or FigValue — is NOT internally
// synchronized: never call two functions on the same handle concurrently;
// serialize access externally if you must share one across threads.
//
// Borrowed buffers: functions that return bytes through (out_ptr, out_len) do
// not transfer ownership — the bytes are borrowed from the handle. There are two
// lifetime classes:
//   * Source-borrowing reads (fig_node_string/_number/_extended,
//     fig_keyvalue_*): valid until fig_document_destroy on that document.
//   * Rendered-buffer borrows (fig_document_serialize, fig_value_serialize[_opts],
//     fig_editor_source, fig_embed_render): each handle holds ONE reused output
//     buffer, so the returned pointer is invalidated by the next call that
//     rewrites it — the next serialize/render on that handle, or (for the editor/
//     embed) the next mutation — and by destroying the handle. Copy the bytes
//     out before that point if you need to keep them.
// Buffers from fig_alloc are the only ones the caller owns; release them (only
// them) with fig_free.
//
// fig_alloc/fig_free exist chiefly to bridge an address-space boundary: a caller
// that does not share the library's memory (the WebAssembly build, driven from
// JavaScript) cannot otherwise place input bytes where the API can read them, nor
// hold output past the next call without copying it somewhere it owns. A host in
// the same address space (the usual native C case) can just use its own
// allocator instead — every API input is copied internally and every output is
// borrowed, so nothing here returns a caller-owned buffer on its own.
//
// fig_free takes the length back: it is a SIZED free, so pass the exact `len` you
// requested from fig_alloc (the (out_ptr,out_len) reads give you that length for
// any bytes you copied into a fig_alloc buffer). A null pointer or zero length is
// a no-op on both.
// ============================================================================

// Allocate `len` bytes from the library's allocator, or return NULL on failure
// or a zero-length request. The bytes are uninitialized. Release with fig_free.
uint8_t *fig_alloc(size_t len);
// Release a buffer obtained from fig_alloc. `len` MUST equal the length passed to
// fig_alloc. A null pointer or zero length is a no-op.
void fig_free(uint8_t *ptr, size_t len);

// Forward compatibility: later fig releases may add enumerators to the enums
// below (status codes, node kinds, extended-scalar kinds, formats). Treat any
// value you do not recognize as opaque — for FigStatus, as a generic failure;
// for the kind enums, as "unknown" — rather than asserting the set is closed.
// Language bindings must not decode a returned value into a fixed enum type
// without a fallback, since an out-of-range discriminant is undefined behavior
// in some languages.
typedef enum FigStatus {
    FIG_STATUS_OK = 0,
    FIG_STATUS_INVALID_ARGUMENT = 1,
    FIG_STATUS_PARSE_ERROR = 2,
    FIG_STATUS_OUT_OF_MEMORY = 3,
    FIG_STATUS_UNSUPPORTED_FORMAT = 4,
    FIG_STATUS_NOT_FOUND = 5,
    // The operation is not defined for these arguments, though each argument is
    // individually valid — as distinct from FIG_STATUS_INVALID_ARGUMENT (a
    // malformed call) and FIG_STATUS_UNSUPPORTED_FORMAT (a format this build
    // cannot handle). Added in core 2.7.0; see fig_embed_retype and
    // fig_editor_uncomment_leading (lines that turned out not to be an entry).
    FIG_STATUS_UNSUPPORTED_OPERATION = 6,
    FIG_STATUS_INTERNAL_ERROR = 255,
} FigStatus;

// Every member of a full build is accepted by fig_parse, the editor
// (fig_editor_*) and the serializers (JSONC = plain-JSON syntax with comments).
// A format compiled out of this build returns FIG_STATUS_UNSUPPORTED_FORMAT
// everywhere; to query the matrix programmatically, call
// fig_format_capabilities below.
//
// Value 6 was FIG_FORMAT_XML, the generic XML fold, through core 2.x. It was
// removed in core 3.0 (ABI 2) and the value is retired: it will never be
// reused, so a caller that still passes it gets FIG_STATUS_UNSUPPORTED_FORMAT
// (or 0 from fig_format_capabilities) rather than some other format.
typedef enum FigFormat {
    FIG_FORMAT_JSON = 1,
    FIG_FORMAT_JSONC = 2,
    FIG_FORMAT_YAML = 3,
    FIG_FORMAT_TOML = 4,
    FIG_FORMAT_ZON = 5,
    FIG_FORMAT_JSON5 = 7,
    // The native `fig` authoring dialect (see src/languages/fig/DESIGN.md).
    // Appended (not inserted) to keep the ABI values of the existing members
    // stable, same as FIG_FORMAT_JSON5 before it.
    FIG_FORMAT_FIG = 8,
    // INI (`[section]` + `key = value`). Read/edit/serialize. Untyped-string
    // scalars: `port = 8080` reads back as the string "8080".
    FIG_FORMAT_INI = 9,
    // dotenv / `.env` (flat `KEY=value`). Read/edit/serialize. Flat string map
    // only — no nesting, untyped scalars (a nested tree cannot be represented;
    // serialize surfaces a diagnostic).
    FIG_FORMAT_DOTENV = 10,
    // Java `.properties` (flat `key=value`). Read/edit/serialize. Same flat,
    // untyped representational limits as dotenv.
    FIG_FORMAT_PROPERTIES = 11,
    // Apple XML property list. Read/edit/serialize. Genuinely typed and nested
    // (dict/array/string/integer/real/bool, date/data via the extended scalar).
    FIG_FORMAT_PLIST = 12,
    // NestedText (https://nestedtext.org). Read/edit/serialize. Nested
    // (dict/list) but deliberately untyped — every leaf is a string.
    FIG_FORMAT_NESTEDTEXT = 13,
} FigFormat;

// Format integers at or above this value name a language registered at
// runtime rather than compiled in. They are assigned per process, in
// registration order, and are never pinned here: a caller that persists a
// format persists its name. Every compiled-in FIG_FORMAT_* enumerator is below
// it, and always will be. Reserved in core 3.0 (ABI 2) ahead of the
// registration entry points, which are a later minor.
#define FIG_FORMAT_RUNTIME_BASE 4096

// Capability bits, OR-combined in the return of fig_format_capabilities.
typedef enum FigCapability {
    FIG_CAP_READ      = 1u << 0, // fig_parse accepts this format
    FIG_CAP_EDIT      = 1u << 1, // fig_editor_*/fig_embed_* accept this format
    FIG_CAP_SERIALIZE = 1u << 2, // fig_*_serialize can write this format
} FigCapability;

// Bitmask of FIG_CAP_* describing what this build can do with `format`. Reflects
// both inherent support (the format's own capability declaration) and
// build-time gating: a format compiled out of this build, or an unknown
// `format` value, reports 0.
uint32_t fig_format_capabilities(int format);

typedef struct FigDocument FigDocument;

// Parse `input[0..input_len]` as `format` into a new document (released with
// fig_document_destroy). Empty input (input_len == 0, with or without a null
// `input`) is handed to the parser and judged per format, NOT rejected up front:
// YAML treats it as a null document and TOML as an empty table (both succeed),
// while JSON/JSON5/ZON require a value/root and return FIG_STATUS_PARSE_ERROR.
// A null `input` with a nonzero `input_len` is FIG_STATUS_INVALID_ARGUMENT.
FigStatus fig_parse(
    const uint8_t *input,
    size_t input_len,
    int format,
    FigDocument **out_doc
);

// Caller-allocated diagnostic for a parse failure. Because the caller owns it
// (on the stack, one per thread), it needs no allocation and has no lifetime
// tied to a handle — which is the whole point: a parse failure happens BEFORE a
// document handle exists, so there is nothing to borrow a message from.
//
// `size` is the version tag, exactly like FigSerializeOptions: set it to
// sizeof(FigError) and the library writes only the fields `size` covers, so the
// struct can gain fields in later releases without breaking an older caller's
// layout. The one frozen dimension is `message` — its capacity cannot grow
// without breaking the ABI — but 256 bytes is ample for a one-line diagnostic,
// and new fields may be appended after it.
//
// On a failed fig_parse_ex the library fills the covered fields; `code` repeats
// the returned FigStatus, `message` is a NUL-terminated human-readable string
// (truncated to fit, `message_len` excludes the NUL). `byte_offset`/`line`/
// `column` locate the failure when known and are 0 ("unknown") otherwise — in
// this release they are always 0 (offset plumbing is a planned follow-up).
typedef struct FigError {
    uint32_t size;          // caller sets sizeof(FigError); see note above
    int      code;          // a FigStatus value (decode unknown as failure)
    size_t   byte_offset;   // offset into input of the failure (0 = unknown)
    uint32_t line;          // 1-based line; 0 = unknown
    uint32_t column;        // 1-based column; 0 = unknown
    size_t   message_len;   // bytes in `message`, excluding the NUL terminator
    char     message[256];  // NUL-terminated, truncated to fit
} FigError;

// As fig_parse, but on failure also fills `out_err` (caller-allocated; set its
// `size` to sizeof(FigError) first) with a diagnostic. `out_err` is nullable:
// passing NULL makes this behave exactly like fig_parse. On FIG_STATUS_OK the
// contents of `out_err` are unspecified — read it only on a nonzero return.
FigStatus fig_parse_ex(
    const uint8_t *input,
    size_t input_len,
    int format,
    FigDocument **out_doc,
    FigError *out_err
);

void fig_document_destroy(FigDocument *doc);

// ============================================================================
// Document traversal (read-only)
//
// Nodes are addressed by id. The sentinel FIG_NODE_NONE means "no such node".
// Pointers returned by the scalar accessors borrow memory owned by the
// document; they remain valid until fig_document_destroy is called on it.
//
// To render a parsed document back out (in its own or another format), see
// fig_document_serialize near the end of this header.
// ============================================================================

typedef uint32_t FigNodeId;
#define FIG_NODE_NONE ((FigNodeId)0xFFFFFFFFu)

typedef enum FigNodeKind {
    FIG_NODE_INVALID  = -1, // null document or out-of-range id
    FIG_NODE_NULL     = 0,
    FIG_NODE_BOOL     = 1,
    FIG_NODE_INT      = 2,
    FIG_NODE_FLOAT    = 3,
    FIG_NODE_STRING   = 4,
    FIG_NODE_SEQUENCE = 5,
    FIG_NODE_MAPPING  = 6,
    FIG_NODE_KEYVALUE = 7,
    FIG_NODE_ALIAS    = 8, // a YAML `*name` alias node (unresolved reference)
} FigNodeKind;

// The node that contains all others. FIG_NODE_NONE for an empty document.
FigNodeId fig_document_root(const FigDocument *doc);

FigNodeKind fig_node_kind(const FigDocument *doc, FigNodeId node);

// Sequence: first element. Mapping: first keyvalue. Otherwise FIG_NODE_NONE.
FigNodeId fig_node_first_child(const FigDocument *doc, FigNodeId node);

// Next element/entry within the containing sequence/mapping, or FIG_NODE_NONE.
FigNodeId fig_node_next_sibling(const FigDocument *doc, FigNodeId node);

// Number of elements (sequence) or entries (mapping); 0 for any other kind.
size_t fig_node_child_count(const FigDocument *doc, FigNodeId node);

// Key/value of a keyvalue node; FIG_NODE_NONE if node is not a keyvalue.
FigNodeId fig_keyvalue_key(const FigDocument *doc, FigNodeId node);
FigNodeId fig_keyvalue_value(const FigDocument *doc, FigNodeId node);

// Scalar accessors. Each returns true and writes its out-param(s) when the
// node has the matching kind; otherwise returns false and leaves them
// untouched. The number accessor yields the raw source text (use
// fig_node_kind to distinguish integer from float).
bool fig_node_bool(const FigDocument *doc, FigNodeId node, bool *out);
bool fig_node_number(const FigDocument *doc, FigNodeId node,
                     const uint8_t **out_ptr, size_t *out_len);
bool fig_node_string(const FigDocument *doc, FigNodeId node,
                     const uint8_t **out_ptr, size_t *out_len);

// Format-specific extended scalar (TOML datetime, ZON enum/char literal).
// Returns true and writes its FigExtKind to *out_kind and source text to
// *out_ptr/*out_len when node is extended; otherwise returns false. Note that
// fig_node_kind still reports such nodes as STRING (datetime / enum literal) or
// INT (char literal), and fig_node_string/fig_node_number still yield the text;
// use this accessor to tell a true string/int apart from an extended scalar.
bool fig_node_extended(const FigDocument *doc, FigNodeId node, int *out_kind,
                       const uint8_t **out_ptr, size_t *out_len);

// ============================================================================
// Editing (write path)
//
// Edits splice only the bytes of the targeted node, preserving comments,
// formatting, and key order everywhere else. A node is addressed by a path: an
// array of segments, each either a mapping key (kind 0) or a sequence index
// (kind 1). Replacement/value bytes are supplied already serialized (e.g. a
// scalar, or multi-line block text indented from column 0); the editor
// re-frames indentation and flow/block context at the splice site.
// ============================================================================

typedef struct FigPathSegment {
    int32_t kind;          // 0 = mapping key, 1 = sequence index
    const uint8_t *key_ptr; // key bytes when kind == 0
    size_t key_len;
    size_t index;          // element index when kind == 1
} FigPathSegment;

// A borrowed UTF-8 string slice: ptr[0..len]. Used for the key list of the
// *_reorder_keys functions.
typedef struct FigStr {
    const uint8_t *ptr;
    size_t len;
} FigStr;

typedef struct FigEditor FigEditor;

// Create an editor over a copy of `input` in the given format. The handle owns
// the source and must be released with fig_editor_destroy.
FigStatus fig_editor_create(const uint8_t *input, size_t input_len,
                            int format, FigEditor **out_editor);
void fig_editor_destroy(FigEditor *editor);

// In-place edits. `path`/`path_len` address the target node (empty path = root
// for inserts/appends). For inserts/appends the path names the container; for
// delete it names the key; for sequence ops it names the sequence.
FigStatus fig_editor_replace_val(FigEditor *editor, const FigPathSegment *path,
                                 size_t path_len, const uint8_t *repl, size_t repl_len);
FigStatus fig_editor_replace_key(FigEditor *editor, const FigPathSegment *path,
                                 size_t path_len, const uint8_t *repl, size_t repl_len);
// Upsert: replace the value at `path`, or—when only the trailing key is absent—
// insert it as a new mapping entry. `path` must end in a key (a path ending in a
// sequence index returns FIG_STATUS_INVALID_ARGUMENT); only the final leaf is
// created, a missing parent container returns FIG_STATUS_NOT_FOUND.
FigStatus fig_editor_set(FigEditor *editor, const FigPathSegment *path,
                         size_t path_len, const uint8_t *val, size_t val_len);
// Comment editing. The marker (`#` for YAML, `//` for JSONC/JSON5) is supplied
// by the editor; strict JSON has no comment syntax and returns
// FIG_STATUS_UNSUPPORTED_FORMAT. `add_leading` inserts an own-line comment above
// the node at `path` (multi-line `text` => one line each); `set_trailing` sets
// the value's same-line comment, replacing any existing one (single-line `text`,
// else FIG_STATUS_INVALID_ARGUMENT). The delete ops remove the leading block /
// the trailing comment, and are a no-op (FIG_STATUS_OK) when there is none.
// An element or entry of a ONE-LINE flow collection (`members = ["a", "b"]`,
// `nested: {k: v}`) shares its parent's line and so owns neither the block above
// it nor its end: the two add/set ops return FIG_STATUS_INVALID_ARGUMENT there,
// the deletes are a no-op, and the reads below answer FIG_STATUS_NOT_FOUND.
FigStatus fig_editor_add_leading_comment(FigEditor *editor, const FigPathSegment *path,
                                         size_t path_len, const uint8_t *text, size_t text_len);
FigStatus fig_editor_set_trailing_comment(FigEditor *editor, const FigPathSegment *path,
                                          size_t path_len, const uint8_t *text, size_t text_len);
FigStatus fig_editor_delete_leading_comments(FigEditor *editor, const FigPathSegment *path,
                                             size_t path_len);
FigStatus fig_editor_delete_trailing_comment(FigEditor *editor, const FigPathSegment *path,
                                             size_t path_len);
// Read a comment back without mutating. `get_leading` returns the own-line block
// above `path` (lines joined by '\n'); `get_trailing` returns the value's
// same-line comment. The marker (and one following space) is stripped. On
// FIG_STATUS_OK the bytes are borrowed from the editor handle (valid until the
// next get on this handle or fig_editor_destroy); `out_len == 0` means a present
// but empty comment (a bare `#`/`//`). FIG_STATUS_NOT_FOUND means no such comment
// exists; strict JSON returns FIG_STATUS_UNSUPPORTED_FORMAT.
FigStatus fig_editor_get_leading_comment(FigEditor *editor, const FigPathSegment *path,
                                         size_t path_len,
                                         const uint8_t **out_ptr, size_t *out_len);
FigStatus fig_editor_get_trailing_comment(FigEditor *editor, const FigPathSegment *path,
                                          size_t path_len,
                                          const uint8_t **out_ptr, size_t *out_len);
// The DANGLING anchor: the comment run at the END of a container's body, after
// its last entry (the third anchor beside leading and trailing; `path` empty =
// the root). Written at the body's child depth, read back with markers and
// indentation stripped, same `not_found`-means-absent and borrowed-bytes rules
// as the reads above. FIG_STATUS_INVALID_ARGUMENT when `path` names a scalar,
// or a flow container with no line for the run to sit on (`{ "a": 1 }`).
FigStatus fig_editor_add_dangling_comment(FigEditor *editor, const FigPathSegment *path,
                                          size_t path_len, const uint8_t *text, size_t text_len);
FigStatus fig_editor_delete_dangling_comments(FigEditor *editor, const FigPathSegment *path,
                                              size_t path_len);
FigStatus fig_editor_get_dangling_comment(FigEditor *editor, const FigPathSegment *path,
                                          size_t path_len,
                                          const uint8_t **out_ptr, size_t *out_len);
// Comment an entry out, and back. `comment_out` prefixes every line of the node
// at `path` with the line marker at that line's own indentation: the entry
// becomes the leading block of what followed it, or — when it was last — the
// parent's dangling run, and the tree no longer has the node. Its own leading
// block stays above it. The uncomment pair is the inverse: strip the marker (and
// one following space) from `line_count` lines of the named block, starting at
// `first_line` (0-based within that block), and reparse. Which lines are an
// entry is the caller's judgement; the editor guarantees the byte edit and that
// it parsed. If the result does not parse, or parses to a document whose other
// nodes changed, the splice is rolled back — the document is unchanged — and the
// call returns the parse error or FIG_STATUS_UNSUPPORTED_OPERATION.
// FIG_STATUS_INVALID_ARGUMENT for the root, and for a node that does not have
// its lines to itself (an item of `[a, b]`, an entry of `{ "a": 1, "b": 2 }`);
// FIG_STATUS_NOT_FOUND when the block has fewer lines than asked for.
FigStatus fig_editor_comment_out(FigEditor *editor, const FigPathSegment *path, size_t path_len);
FigStatus fig_editor_uncomment_leading(FigEditor *editor, const FigPathSegment *path,
                                       size_t path_len, size_t first_line, size_t line_count);
FigStatus fig_editor_uncomment_dangling(FigEditor *editor, const FigPathSegment *path,
                                        size_t path_len, size_t first_line, size_t line_count);
FigStatus fig_editor_insert_key(FigEditor *editor, const FigPathSegment *path, size_t path_len,
                                const uint8_t *key, size_t key_len,
                                const uint8_t *val, size_t val_len);
FigStatus fig_editor_delete_key(FigEditor *editor, const FigPathSegment *path, size_t path_len);
FigStatus fig_editor_append_seq(FigEditor *editor, const FigPathSegment *path, size_t path_len,
                                const uint8_t *val, size_t val_len);
FigStatus fig_editor_prepend_seq(FigEditor *editor, const FigPathSegment *path, size_t path_len,
                                 const uint8_t *val, size_t val_len);
FigStatus fig_editor_remove_seq_item(FigEditor *editor, const FigPathSegment *path,
                                     size_t path_len, size_t index);
// Move the mapping entry at `src_path` to immediately before the entry at
// `dest_path` (both must name keys in the same mapping). Reorder the entries of
// the mapping at `path` so `keys` come first in order, the rest following in
// original order; unknown keys are ignored. Owned comments travel with entries.
FigStatus fig_editor_move_key(FigEditor *editor,
                              const FigPathSegment *src_path, size_t src_path_len,
                              const FigPathSegment *dest_path, size_t dest_path_len);
FigStatus fig_editor_reorder_keys(FigEditor *editor, const FigPathSegment *path, size_t path_len,
                                  const FigStr *keys, size_t keys_len);
// Move the sequence item at index `from` to index `to` (array-move semantics).
// Reorder the items of the sequence at `path` so the items at `indices` come
// first in order, the rest following in original order; out-of-range indices
// are ignored. Block items carry owned comments; flow sequences keep separators.
FigStatus fig_editor_move_item(FigEditor *editor, const FigPathSegment *path, size_t path_len,
                               size_t from, size_t to);
FigStatus fig_editor_reorder_items(FigEditor *editor, const FigPathSegment *path, size_t path_len,
                                   const size_t *indices, size_t indices_len);
// Reconcile the sequence at `path` so its items are exactly `items` (each an
// already-serialized scalar value in the document's format), preserving the
// comments on items that survive. Items are matched to the current items by
// value (kind + value, honoring multiplicity), so a kept or reordered item
// keeps its comments; only genuinely new values are inserted and only dropped
// values are deleted. The result order matches `items`. The compound edit is
// atomic. Declines with FIG_STATUS_INVALID_ARGUMENT when it cannot safely diff
// the shape (empty `items`, an empty current list, a non-scalar item on either
// side, or a format whose scalars can't stand alone, e.g. TOML); the caller
// should then replace the whole value instead. A non-sequence target is also
// FIG_STATUS_INVALID_ARGUMENT.
FigStatus fig_editor_set_sequence(FigEditor *editor, const FigPathSegment *path, size_t path_len,
                                  const FigStr *items, size_t items_len);

// Whole-container ops, for containers that are SCATTERED through the source: a
// TOML [header] table (whose body is the lines after it, extended by every
// [a.b] header elsewhere in the file), an INI [section], a fig block container.
// Such a container owns no single range to splice, so the key ops above cannot
// address it — fig_editor_delete_key and fig_editor_replace_val at a table path
// answer FIG_STATUS_INVALID_ARGUMENT, and these are where that request goes.
//
// Formats differ in which they support: TOML all six, INI and fig the
// delete/move/reorder three, and every other format none — YAML, JSON and the
// rest nest their containers in one contiguous region, so the key ops already
// handle them. An op a format does not support answers
// FIG_STATUS_UNSUPPORTED_FORMAT.
//
// `body` is verbatim entry lines in the document's format (e.g. `ip = "10.0.0.1"\n`
// for TOML), spliced and reparsed like every other edit — a body that does not
// parse rolls the document back and returns FIG_STATUS_PARSE_ERROR.
FigStatus fig_editor_delete_container(FigEditor *editor, const FigPathSegment *path,
                                      size_t path_len);
FigStatus fig_editor_insert_container(FigEditor *editor, const FigPathSegment *path,
                                      size_t path_len,
                                      const uint8_t *body, size_t body_len);
// Rename the container at `path` to `new_leaf`, rewriting EVERY line that names
// it: renaming `a` rewrites [a], [a.b] and [[a.c]] alike.
FigStatus fig_editor_rename_container(FigEditor *editor, const FigPathSegment *path,
                                      size_t path_len,
                                      const uint8_t *new_leaf, size_t new_leaf_len);
// Move the container at `src_path` before the one at `dest_path`, re-emitting
// its scattered fragments contiguously; interleaved foreign containers stay put.
// A NULL `dest_path` means "to the end of the document" — distinct from a
// zero-length path, which every other entry point reads as the root.
FigStatus fig_editor_move_container(FigEditor *editor,
                                    const FigPathSegment *src_path, size_t src_path_len,
                                    const FigPathSegment *dest_path, size_t dest_path_len);
// Reorder top-level containers so those named in `order` come first, in that
// order, each re-emitted contiguously at the position the earliest of them
// currently occupies. Containers not named keep their places.
FigStatus fig_editor_reorder_containers(FigEditor *editor,
                                        const FigStr *order, size_t order_len);
// Append a new element with body `body` to the container sequence at `path` —
// TOML's [[header]] array-of-tables append.
FigStatus fig_editor_append_container_to_seq(FigEditor *editor, const FigPathSegment *path,
                                             size_t path_len,
                                             const uint8_t *body, size_t body_len);

// Borrow the editor's current source bytes. Valid until the next mutation or
// fig_editor_destroy.
FigStatus fig_editor_source(const FigEditor *editor,
                            const uint8_t **out_ptr, size_t *out_len);

// ============================================================================
// Embedded regions (e.g. markdown frontmatter)
// ============================================================================

typedef struct FigSpan { size_t start; size_t end; } FigSpan;
// `size` is the version tag, exactly like FigError/FigSerializeOptions: set it to
// sizeof(FigRegion) before calling fig_embed_extract and the library writes only
// the fields `size` covers, so the struct can gain trailing fields in later
// releases without disturbing an older caller's layout. A field past `size` is
// left untouched.
typedef struct FigRegion {
    uint32_t size;          // caller sets sizeof(FigRegion); see note above
    FigSpan open_fence;
    FigSpan content;
    FigSpan close_fence;
    // The host body outside the fences (in host-file coordinates): the suffix
    // after the close fence for frontmatter, the prefix before the open fence
    // for endmatter. The read-side twin of `content` (frontmatter vs. body),
    // and one-sided: for a mid-document block (an HTML <script> data island)
    // it names only the text AFTER the block. Prefer the two spans below when
    // reassembling the file.
    FigSpan body;
    // The host text on each side of the block: [0, open_fence.start) and
    // [close_fence.end, input_len). Together with the three region spans they
    // tile the input exactly — every byte in exactly one span, a leading UTF-8
    // BOM at the head of `body_before` — so a caller can rebuild the host
    // without losing a byte, whichever side the block sits on.
    //
    // Added in core 2.7.0. A caller whose `size` predates these fields is not
    // written past its own layout; see the `size` note above.
    FigSpan body_before;
    FigSpan body_after;
} FigRegion;

// The container half of an embed selector. Every fig_embed_* entry point that
// selects a region takes a (container, format) pair: this enum and a FigFormat.
// The four PARAMETRIC containers hold any format with an embedded spelling
// (JSON, YAML, TOML and fig today; FIG_STATUS_INVALID_ARGUMENT for one without,
// such as INI, and for an unknown value of either integer). The three PRESET
// containers pin their own format and ignore the format argument; detection
// reports the pinned one, so a pair read back from fig_embed_detect is always
// meaningful on its own.
//
// ABI 2 shape. ABI 1 (core 2.4 - 2.9) flattened the pair into one FigEmbedType
// enum of nineteen products; each value maps onto exactly one pair, e.g.
// FIG_EMBED_FRONTMATTER_YAML -> (FIG_EMBED_MD_FRONTMATTER, FIG_FORMAT_YAML),
// FIG_EMBED_FRONTMATTER_JSON -> (FIG_EMBED_SEMICOLONS_JSON, any),
// FIG_EMBED_FRONTMATTER_FIG -> (FIG_EMBED_FENCED, FIG_FORMAT_FIG),
// FIG_EMBED_FENCED_TOML -> (FIG_EMBED_FENCED, FIG_FORMAT_TOML), and so on down
// the MD_FRONTMATTER_*, HTML_SCRIPT_* and HTML_CODE_* groups.
typedef enum FigEmbedContainer {
    // ---<lang> ... --- (or ...) markdown frontmatter. A bare --- is YAML, so
    // (FIG_EMBED_MD_FRONTMATTER, FIG_FORMAT_YAML) is the classic frontmatter.
    FIG_EMBED_MD_FRONTMATTER = 0,
    // ```<lang> ... ``` fenced block.
    FIG_EMBED_FENCED = 1,
    // <script type="application/<lang>"> ... </script> HTML data island.
    FIG_EMBED_HTML_SCRIPT = 2,
    // <pre><code class="language-<lang>"> ... </code></pre> visible code block.
    // Content is entity-encoded; the editing handle decodes on open and
    // re-encodes span-aware on render, so an edit keeps every untouched byte's
    // original encoding while canonically encoding only what changed.
    FIG_EMBED_HTML_CODE = 3,
    // ;;; ... ;;; JSON frontmatter. Preset: format argument ignored (JSON).
    FIG_EMBED_SEMICOLONS_JSON = 4,
    // +++ ... +++ TOML frontmatter (Hugo/Zola). Preset: format ignored (TOML).
    FIG_EMBED_PLUS_TOML = 5,
    // A trailing ```endmatter ... ``` YAML block. Preset: format ignored (YAML).
    FIG_EMBED_ENDMATTER_YAML = 6,
} FigEmbedContainer;

// Locate an embedded region and report its fence/content/body spans (in
// host-file coordinates) without parsing the content. The caller must set
// `out_region->size = sizeof(FigRegion)` first (see FigRegion). FIG_STATUS_NOT_FOUND
// when no region of that type exists; a region whose open fence has no matching
// close is FIG_STATUS_PARSE_ERROR.
FigStatus fig_embed_extract(const uint8_t *input, size_t input_len,
                            int container, int format, FigRegion *out_region);

// Best-effort sniff of which embed archetype `input` uses: try each known
// archetype's OPEN delimiter and report the first that matches. Only the open
// delimiter is checked — an unterminated block is still recognized as its
// archetype, so a follow-up fig_embed_extract/fig_embed_open surfaces the real
// FIG_STATUS_PARSE_ERROR instead of a misleading "nothing found". Writes the
// detected FigEmbedContainer and FigFormat to `out_container` and `out_format`
// and returns FIG_STATUS_OK, or FIG_STATUS_NOT_FOUND when `input` opens none of
// them (both out params are left untouched). A preset container reports the
// format it pins. Detection is delimiter-only, so it works regardless of which
// inner formats this build compiles in.
FigStatus fig_embed_detect(const uint8_t *input, size_t input_len,
                           int *out_container, int *out_format);

// Re-house `input`'s embedded region under a DIFFERENT archetype's fences: keep
// every host byte outside the block, and wrap `content` — the already
// re-serialized inner document, in the TARGET archetype's inner format — in the
// target's convention. The stateless counterpart of fig_embed_extract, and the
// splice half of "convert this file's embed style": the caller does the format
// conversion (fig_value_serialize, or its own printer), fig does the fences and
// the placement.
//
// The block MOVES only when the target puts it at the other end of the file
// (frontmatter <-> endmatter); otherwise it is re-housed exactly where it sat,
// so a same-archetype retype is a byte-identical rebuild. The host text on both
// sides survives in file order either way, and a UTF-8 BOM is re-emitted at
// offset 0 rather than travelling with the prose it precedes.
//
// Moving a MID-DOCUMENT block (FIG_EMBED_HTML_SCRIPT, FIG_EMBED_HTML_CODE)
// to an archetype that sits at an edge of the file returns
// FIG_STATUS_UNSUPPORTED_OPERATION: hoisting a `---` fence above <html> is
// neither valid markdown nor valid HTML, and leaving the block where it is does
// not make it frontmatter. Converting such a file means converting the host too.
// Mid-document to mid-document is fine, and splices in place.
//
// FIG_STATUS_NOT_FOUND when `input` has no region of the `from` pair;
// FIG_STATUS_PARSE_ERROR when it opens one and never closes it.
//
// OWNERSHIP: on FIG_STATUS_OK the result is a freshly allocated buffer the
// CALLER owns — release it with fig_free(ptr, len), passing back the exact
// *out_len. This is the one fig call that hands back an owned buffer rather than
// one borrowed from a handle, because it holds no handle. Nothing is written to
// the out params on failure.
//
// Added in core 2.7.0; takes (container, format) pairs since core 3.0.
FigStatus fig_embed_retype(const uint8_t *input, size_t input_len,
                           int from_container, int from_format,
                           int to_container, int to_format,
                           const uint8_t *content, size_t content_len,
                           uint8_t **out_ptr, size_t *out_len);

// ============================================================================
// Embed editor (combined): opens the config inside a host file — selected by a
// (FigEmbedContainer, FigFormat) pair — and edits it in its inner format,
// leaving the fences and surrounding host text byte-identical. fig_embed_open
// picks the inner editor from the pair; the edit ops mirror fig_editor_*. A
// pair whose format is compiled out of this build is
// FIG_STATUS_UNSUPPORTED_FORMAT; one the model cannot spell (see
// FigEmbedContainer) is FIG_STATUS_INVALID_ARGUMENT.
// ============================================================================

typedef struct FigEmbed FigEmbed;

FigStatus fig_embed_open(const uint8_t *input, size_t input_len, int container, int format, FigEmbed **out_embed);
// Like fig_embed_open, but when no region of that pair exists, create an empty
// one (frontmatter at the top, endmatter at the bottom) instead of returning
// FIG_STATUS_NOT_FOUND — so a subsequent fig_embed_set / fig_embed_insert_key
// lands the first entry. An existing region is opened unchanged; a malformed one
// (open fence with no close) still fails.
FigStatus fig_embed_open_or_init(const uint8_t *input, size_t input_len, int container, int format, FigEmbed **out_embed);
void fig_embed_destroy(FigEmbed *embed);

FigStatus fig_embed_replace_val(FigEmbed *embed, const FigPathSegment *path,
                                size_t path_len, const uint8_t *repl, size_t repl_len);
FigStatus fig_embed_replace_key(FigEmbed *embed, const FigPathSegment *path,
                                size_t path_len, const uint8_t *repl, size_t repl_len);
// Upsert on the embedded config (mirrors fig_editor_set): replace the value at
// `path`, or insert it when only the trailing key is absent. `path` must end in
// a key.
FigStatus fig_embed_set(FigEmbed *embed, const FigPathSegment *path,
                        size_t path_len, const uint8_t *val, size_t val_len);
// Comment editing on the embedded config (mirrors fig_editor_*; YAML frontmatter
// uses `#`, JSON frontmatter is strict JSON and rejects comments).
FigStatus fig_embed_add_leading_comment(FigEmbed *embed, const FigPathSegment *path,
                                        size_t path_len, const uint8_t *text, size_t text_len);
FigStatus fig_embed_set_trailing_comment(FigEmbed *embed, const FigPathSegment *path,
                                         size_t path_len, const uint8_t *text, size_t text_len);
FigStatus fig_embed_delete_leading_comments(FigEmbed *embed, const FigPathSegment *path,
                                            size_t path_len);
FigStatus fig_embed_delete_trailing_comment(FigEmbed *embed, const FigPathSegment *path,
                                            size_t path_len);
// Read a comment from the embedded config (mirrors fig_editor_get_*): borrowed
// bytes on OK, FIG_STATUS_NOT_FOUND when absent, len 0 when present-but-empty.
FigStatus fig_embed_get_leading_comment(FigEmbed *embed, const FigPathSegment *path,
                                        size_t path_len,
                                        const uint8_t **out_ptr, size_t *out_len);
FigStatus fig_embed_get_trailing_comment(FigEmbed *embed, const FigPathSegment *path,
                                         size_t path_len,
                                         const uint8_t **out_ptr, size_t *out_len);
// The dangling anchor and the comment-out pair on the embedded config (mirrors
// fig_editor_*; see those for the semantics and the failure modes).
FigStatus fig_embed_add_dangling_comment(FigEmbed *embed, const FigPathSegment *path,
                                         size_t path_len, const uint8_t *text, size_t text_len);
FigStatus fig_embed_delete_dangling_comments(FigEmbed *embed, const FigPathSegment *path,
                                             size_t path_len);
FigStatus fig_embed_get_dangling_comment(FigEmbed *embed, const FigPathSegment *path,
                                         size_t path_len,
                                         const uint8_t **out_ptr, size_t *out_len);
FigStatus fig_embed_comment_out(FigEmbed *embed, const FigPathSegment *path, size_t path_len);
FigStatus fig_embed_uncomment_leading(FigEmbed *embed, const FigPathSegment *path,
                                      size_t path_len, size_t first_line, size_t line_count);
FigStatus fig_embed_uncomment_dangling(FigEmbed *embed, const FigPathSegment *path,
                                       size_t path_len, size_t first_line, size_t line_count);
FigStatus fig_embed_insert_key(FigEmbed *embed, const FigPathSegment *path, size_t path_len,
                               const uint8_t *key, size_t key_len,
                               const uint8_t *val, size_t val_len);
FigStatus fig_embed_delete_key(FigEmbed *embed, const FigPathSegment *path, size_t path_len);
FigStatus fig_embed_append_seq(FigEmbed *embed, const FigPathSegment *path, size_t path_len,
                               const uint8_t *val, size_t val_len);
FigStatus fig_embed_prepend_seq(FigEmbed *embed, const FigPathSegment *path, size_t path_len,
                                const uint8_t *val, size_t val_len);
FigStatus fig_embed_remove_seq_item(FigEmbed *embed, const FigPathSegment *path,
                                    size_t path_len, size_t index);
FigStatus fig_embed_move_key(FigEmbed *embed,
                             const FigPathSegment *src_path, size_t src_path_len,
                             const FigPathSegment *dest_path, size_t dest_path_len);
FigStatus fig_embed_reorder_keys(FigEmbed *embed, const FigPathSegment *path, size_t path_len,
                                 const FigStr *keys, size_t keys_len);
FigStatus fig_embed_move_item(FigEmbed *embed, const FigPathSegment *path, size_t path_len,
                              size_t from, size_t to);
FigStatus fig_embed_reorder_items(FigEmbed *embed, const FigPathSegment *path, size_t path_len,
                                  const size_t *indices, size_t indices_len);
// Comment-preserving sequence reconcile (see fig_editor_set_sequence).
FigStatus fig_embed_set_sequence(FigEmbed *embed, const FigPathSegment *path, size_t path_len,
                                 const FigStr *items, size_t items_len);

// Replace the host BODY (the prose the config is embedded in) with `body`,
// keeping the fences and the current (possibly edited) content byte-identical.
// The body is the suffix after the close fence (frontmatter) or the prefix
// before the open fence (endmatter); only that side is swapped. `body` is taken
// verbatim (not parsed) and copied; an empty `body` clears it. Composes with the
// value edits — edit keys, replace the body, then render once. Takes effect at
// the next fig_embed_render.
FigStatus fig_embed_replace_body(FigEmbed *embed, const uint8_t *body, size_t body_len);

// Render the full host file with the edited embed. Borrowed bytes, valid
// until the next call or fig_embed_destroy.
FigStatus fig_embed_render(FigEmbed *embed, const uint8_t **out_ptr, size_t *out_len);

// ============================================================================
// Value construction + serialization
//
// The build/serialize counterpart to the read-side traversal API. Construct a
// fresh value tree node-by-node, then render it to any supported format. A
// built value owns no source, so every input byte is copied — caller buffers
// need not outlive the calls.
//
// Construction is bottom-up: build child nodes first, then the container from
// their ids. Each builder call returns the new node's id via *out_id. A node id
// must be placed in exactly one container (a node carries a single sibling
// link). Ids handed to fig_value_seq/fig_value_map must name already-created
// nodes, else FIG_STATUS_INVALID_ARGUMENT.
// ============================================================================

typedef struct FigValue FigValue;

// A key: value entry for fig_value_map; both name nodes created earlier.
typedef struct FigKeyValue {
    FigNodeId key;
    FigNodeId value;
} FigKeyValue;

// Format-specific scalar kinds (TOML datetimes, ZON enum/char literals, JSON5
// non-finite numbers, plist dates and data). Mirrors the core's `ExtKind`;
// `zig build abi-check` holds this enum, the Rust `ExtKind` and the TypeScript
// `ExtKind` to it name-for-name.
typedef enum FigExtKind {
    FIG_EXT_OFFSET_DATETIME = 0,
    FIG_EXT_LOCAL_DATETIME  = 1,
    FIG_EXT_LOCAL_DATE      = 2,
    FIG_EXT_LOCAL_TIME      = 3,
    FIG_EXT_ENUM_LITERAL    = 4,
    FIG_EXT_CHAR_LITERAL    = 5,
    FIG_EXT_NUMBER_SPECIAL  = 6,
    // A plist <date>: the raw ISO-8601 timestamp, verbatim.
    FIG_EXT_PLIST_DATE      = 7,
    // A plist <data>: the base64 payload with all whitespace stripped.
    FIG_EXT_PLIST_DATA      = 8,
} FigExtKind;

FigStatus fig_value_create(FigValue **out_value);
void fig_value_destroy(FigValue *value);

// Scalars. Each writes the new node's id to *out_id.
FigStatus fig_value_null(FigValue *value, FigNodeId *out_id);
FigStatus fig_value_bool(FigValue *value, bool b, FigNodeId *out_id);
FigStatus fig_value_int(FigValue *value, int64_t n, FigNodeId *out_id);
FigStatus fig_value_uint(FigValue *value, uint64_t n, FigNodeId *out_id);
// A numeric scalar from already-formatted text; is_float records its kind. The
// float entry point (the canonical float-text policy is the caller's for now)
// and the escape hatch for integers outside the int64/uint64 range.
FigStatus fig_value_number(FigValue *value, const uint8_t *raw, size_t raw_len,
                           bool is_float, FigNodeId *out_id);
FigStatus fig_value_string(FigValue *value, const uint8_t *ptr, size_t len, FigNodeId *out_id);
FigStatus fig_value_extended(FigValue *value, int kind, const uint8_t *text, size_t text_len,
                             FigNodeId *out_id);

// Containers, built from already-created child ids.
FigStatus fig_value_seq(FigValue *value, const FigNodeId *items, size_t items_len, FigNodeId *out_id);
FigStatus fig_value_map(FigValue *value, const FigKeyValue *entries, size_t entries_len, FigNodeId *out_id);

// Render the subtree rooted at `root` in `format`. Output bytes are borrowed
// from the value and valid until the next fig_value_serialize or
// fig_value_destroy. A value the target cannot represent (e.g. a null in TOML)
// returns FIG_STATUS_UNSUPPORTED_FORMAT.
FigStatus fig_value_serialize(FigValue *value, FigNodeId root, int format,
                              const uint8_t **out_ptr, size_t *out_len);

// Output style for fig_value_serialize_opts. A NULL options pointer selects the
// defaults shown here (identical output to fig_value_serialize). `pretty` is
// honored by JSON, ZON, and TOML (array wrapping); `indent` by JSON and TOML's
// wrapped arrays; `width` by the inline-vs-expanded layout of TOML, YAML, and
// fig.
typedef struct FigSerializeOptions {
  // Set this to sizeof(FigSerializeOptions). It is the struct's version tag:
  // fig may append fields in later releases, and reads a given field only when
  // `size` is large enough to cover it — so a struct laid out by an older
  // caller still works, with the new fields taking their defaults. A `size`
  // too small to cover a field (e.g. 0 from a zero-initialized struct that
  // forgot to set it) makes that field read as its default, NOT garbage.
  uint32_t size;
  // Nonzero (default): multi-line, indented output. Zero: compact single-line.
  // For TOML, zero keeps every array on one line; nonzero lets a wide array wrap.
  uint8_t pretty;
  // Spaces per indent level when `pretty` is nonzero (JSON, and TOML's wrapped
  // arrays). 0 => default 2.
  uint8_t indent;
  // Nonzero: drop comments carried on the value instead of emitting them. Zero
  // (default): preserve them where the target format allows. Appended after
  // `indent`; older callers (smaller `size`) keep the preserve default.
  uint8_t strip_comments;
  // fig_document_serialize only. Nonzero: preserve values the target format
  // cannot represent natively (a null in TOML, a TOML datetime in JSON, ...)
  // through a $fig envelope, and decode any such envelope found in the source.
  // Zero (default): lossy -- an unrepresentable value yields
  // FIG_STATUS_UNSUPPORTED_FORMAT. Ignored by fig_value_serialize_opts. Appended
  // after `strip_comments`; older callers keep the lossy default.
  uint8_t lossless;
  // TOML, YAML, and fig: the column budget for their inline-vs-expanded layout.
  // A mapping/array that renders within `width` columns stays inline
  // (k = { ... } / [a, b]); a wider one expands to a [section] / a wrapped array
  // / block lines. For YAML the budget governs NESTED containers only -- a root
  // mapping/sequence always renders block.
  // 0 => default 80, NOT "never inline": a zero-initialized struct (with a valid
  // `size`) must not silently mean block everywhere. Pass 1 to force block.
  // Appended after `lossless`; older callers (smaller `size`)
  // keep the 80-column default. uint16_t, so the struct pads to a 12-byte size.
  uint16_t width;
  // fig_value_serialize_opts + FIG_FORMAT_FIG only: nonzero renders a container
  // root as inline *flow* ([a, b] / { k = v }) instead of the block spelling.
  // This is what the bindings' editor splice paths set: a fragment spliced
  // after `key = ` has no valid block spelling in the fig dialect (`* ` element
  // lines and section headers only parse as standalone lines), so flow is the
  // only round-trippable form. Zero (default): unchanged block rendering.
  // Appended after `width`; it occupies what was the 12-byte layout's trailing
  // padding, so sizeof does not change -- a zero-initialized older caller reads
  // as the default (off), per the same forward-compat rule as the fields above.
  uint8_t flow;
} FigSerializeOptions;

// As fig_value_serialize, but `options` (NULL => defaults) controls output style
// such as compact vs. pretty-printed JSON.
FigStatus fig_value_serialize_opts(FigValue *value, FigNodeId root, int format,
                                   const FigSerializeOptions *options,
                                   const uint8_t **out_ptr, size_t *out_len);

// ============================================================================
// Document serialization (cross-format conversion)
//
// Render a whole parsed FigDocument to a writable format — the conversion
// primitive. `format` is any FigFormat this build compiled in (a compiled-out
// one returns FIG_STATUS_UNSUPPORTED_FORMAT). The source may be any parsed
// format (e.g. plist in, JSON out).
//
// When the source is YAML and the target is not, the reference layer (anchors,
// aliases, merge keys, tags) is collapsed automatically (strict tag mode: an
// unknown/custom tag yields FIG_STATUS_UNSUPPORTED_FORMAT). Comments carried on
// the source are preserved where the target format allows.
//
// `options` (NULL => defaults) is the same struct as fig_value_serialize_opts.
// With `lossless` zero (default) the conversion is lossy: a value the target
// cannot represent natively (e.g. a null in TOML) returns
// FIG_STATUS_UNSUPPORTED_FORMAT. With `lossless` nonzero, such values round-trip
// through a $fig envelope and any envelope already in the source is decoded back.
//
// Output bytes are borrowed from `doc` and valid until the next
// fig_document_serialize call on it or fig_document_destroy. Serializes the whole
// document (no subtree selection in this version).
FigStatus fig_document_serialize(FigDocument *doc, int format,
                                 const FigSerializeOptions *options,
                                 const uint8_t **out_ptr, size_t *out_len);

// ==================
// DIAGNOSTICS
// ==================
//
// Report what a serialization would silently lose, without performing it. fig's
// printers degrade or drop data the target format cannot hold (a TOML null
// vanishes, a datetime becomes a string, a block comment becomes a # run, plain
// JSON drops comments) — all still-valid output, so it happens quietly. These
// calls surface each such event so a host can warn, block, or ignore it.

// What kind of loss a warning describes. Mirrors the producing values exactly.
typedef enum FigWarningCode {
  // A carried comment is not emitted at all (no comment syntax, or stripped).
  FIG_WARNING_COMMENT_DROPPED = 0,
  // A block comment is rendered as a run of line comments.
  FIG_WARNING_COMMENT_STYLE_DEGRADED = 1,
  // A node is removed entirely (the target cannot represent it even degraded).
  FIG_WARNING_VALUE_DROPPED = 2,
  // An extended/non-finite value is rendered as a poorer type.
  FIG_WARNING_TYPE_DEGRADED = 3,
} FigWarningCode;

// Why the loss happens — so a host can keep or ignore each class.
typedef enum FigWarningCause {
  // The target format inherently cannot represent it.
  FIG_WARNING_CAUSE_FORMAT_LIMITATION = 0,
  // A caller option forced it (e.g. strip_comments).
  FIG_WARNING_CAUSE_EXPLICIT_OPTION = 1,
} FigWarningCause;

// One lossy event, retrieved by index via fig_*_warning (the diagnose calls
// below report only the count). `code`/`cause` hold FigWarningCode/
// FigWarningCause values (compared as int for forward-compatibility). `path`/
// `note` are NOT null-terminated — use the paired `*_len`. `path` is the
// dotted/[i] location (path_len == 0 means the document root); `note` is the
// degraded-to type for FIG_WARNING_TYPE_DEGRADED (e.g. "string", "number"),
// empty otherwise. Both pointers borrow the producing handle's storage (see the
// borrowing note below).
//
// FigWarning is caller-allocated and `size` is its version tag, the same policy
// FigSerializeOptions/FigError follow: set `size` to sizeof(FigWarning) and the
// library writes only the fields `size` covers, so the struct can gain fields
// without breaking an older caller's layout. (This replaced an earlier
// library-allocated array, which could not grow this way.)
typedef struct FigWarning {
  uint32_t size;        // caller sets sizeof(FigWarning); see note above
  int code;
  int cause;
  const uint8_t *path;
  size_t path_len;
  const uint8_t *note;
  size_t note_len;
} FigWarning;

// Report HOW MANY events serializing the whole parsed document to `format` would
// produce, using the same pipeline fig_document_serialize prints from (YAML
// collapse and, under `options->lossless`, $fig envelopes — so lossless
// suppresses value losses). `options` (NULL => defaults) supplies
// pretty/strip_comments/lossless, which change what is lost. On FIG_STATUS_OK
// writes the event count to *out_count (0 if nothing is lost); retrieve each
// event with fig_document_warning. The computed set is retained on `doc` and
// stays valid (including the `path`/`note` bytes the warnings borrow) until the
// next fig_document_diagnose on it or fig_document_destroy.
FigStatus fig_document_diagnose(FigDocument *doc, int format,
                                const FigSerializeOptions *options,
                                size_t *out_count);

// Copy the event at `index` from the most recent fig_document_diagnose on `doc`
// into caller-allocated `*out` (set out->size to sizeof(FigWarning) first). An
// `index` >= the reported count, or a call with no prior diagnose, returns
// FIG_STATUS_INVALID_ARGUMENT. The `path`/`note` pointers written into `*out`
// borrow `doc` under the lifetime described on fig_document_diagnose.
FigStatus fig_document_warning(FigDocument *doc, size_t index, FigWarning *out);

// Report how many events serializing the built value subtree rooted at `root` to
// `format` would produce. The value builder has no source envelopes, so
// `options->lossless` is ignored here. Retrieve each event with
// fig_value_warning. Retention/borrowing rules match fig_document_diagnose
// (valid until the next fig_value_diagnose on `value` or fig_value_destroy).
FigStatus fig_value_diagnose(FigValue *value, FigNodeId root, int format,
                             const FigSerializeOptions *options,
                             size_t *out_count);

// Copy the event at `index` from the most recent fig_value_diagnose on `value`
// into caller-allocated `*out` (set out->size to sizeof(FigWarning) first).
// Out-of-range `index` or no prior diagnose returns FIG_STATUS_INVALID_ARGUMENT.
FigStatus fig_value_warning(FigValue *value, size_t index, FigWarning *out);

#ifdef __cplusplus
}
#endif
