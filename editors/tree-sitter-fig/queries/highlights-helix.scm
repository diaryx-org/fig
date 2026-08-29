; tree-sitter-fig highlight query — HELIX capture-name conventions.
;
; Same captures as ./highlights.scm, renamed to the scopes Helix themes
; actually define. Kept as a separate file rather than a translation layer
; because the two vocabularies genuinely disagree: Zed's `@property` /
; `@punctuation.list_marker` are not Helix scopes, and Helix's
; `@constant.numeric.integer` / `@variable.other.member` are not Zed's. Node
; names are the grammar's, so any grammar change lands in both files.
;
; Conventions follow Helix's own bundled `toml` and `yaml` queries:
;   - a table/section head is `@type`, a key inside it `@variable.other.member`
;   - numbers are `constant.numeric*`
;   - structural sigils are `@punctuation.special`
;
; Install: symlink or copy to ~/.config/helix/runtime/queries/fig/highlights.scm

; ── comments ──
(comment) @comment

; ── structure markers ──
; `>`-run: depth is the COUNT (see DESIGN.md). Load-bearing, so it gets the
; `special` scope YAML gives `---`/`*` rather than a plain delimiter.
(markers) @punctuation.special
; `*` element marker — the analogue of YAML's `-`, which Helix scopes as a
; delimiter.
(star) @punctuation.delimiter
; `+` continuation line (re-runs the last `[]` append header).
(continuation) @punctuation.special

; ── keys ──
; A header names a container, like a TOML `[table]` → `@type`. An assignment
; key names a field inside one → `@variable.other.member`. Helix's toml query
; draws exactly this distinction.
(header key: (keypath (key) @type))
(assignment key: (keypath (key) @variable.other.member))
(flow_object key: (bare_key) @variable.other.member)
(flow_object key: (string_single) @variable.other.member)
(flow_object key: (string_double) @variable.other.member)

; ── type annotations: `: int`, `: enum`, … ──
; A closed set the parser knows by name, so `builtin` — and it keeps
; annotations distinct from the header keys above, which are also `@type`.
(type) @type.builtin

; ── operators & path/index punctuation ──
"=" @operator
":" @operator
"." @punctuation.delimiter
"," @punctuation.delimiter
["[" "]"] @punctuation.bracket
["{" "}"] @punctuation.bracket

; ── scalars ──
(boolean) @constant.builtin.boolean
(null) @constant.builtin
; one token for hex/octal/binary/float/decimal alike (see grammar.js), so the
; generic parent scope — splitting into .integer/.float would mis-scope half.
(number) @constant.numeric
(integer) @constant.numeric.integer
; a `: int`/`: float` coercion lookalike (`09`, `1.`, `inf`) — see grammar.js
(coerced_number) @constant.numeric
(datetime) @string.special
(string_single) @string
(string_double) @string
(multiline_single) @string
(multiline_double) @string
(bare_string) @string
(flow_bare) @string
; a `: string` sink RHS: verbatim text, brackets included (`: string = [ 1 + 2 ]`)
(string_sink) @string
; balanced-then-trailing bare strings: markdown links, globs, BBCode
; (`[Blog](/x)`, `[a-z]*.md`, `[b]x[/b]`) — see grammar.js and src/scanner.c.
(bracket_led_bare_string) @string
(flow_bracket_led_bare) @string

; a quoted RHS under a non-`string`/non-`int`/non-`float` annotation that
; doesn't accept a quoted spelling (`bool`/`datetime`/`date`/`time`) — this is
; a hard `FigTypeMismatch` in the real parser (spec.md § 5.3), not a valid
; string; see grammar.js `_number_rhs`/`_other_rhs`.
(invalid_annotated_string) @error
