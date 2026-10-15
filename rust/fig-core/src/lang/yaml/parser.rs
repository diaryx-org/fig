//! Port of languages/yaml/src/parser.zig.
//!
//! The parser turns YAML tokens into the node table fig hands it. It writes
//! a row per node as it meets it (`RowWriter`) — a container when it opens,
//! its children as they complete — with each node's tag, anchor, item
//! marker, entry separator and comments beside it, and puts the rows in
//! pre-order once the root is done.
//!
//! A decoded string (a quoted or folded scalar) is copied into the table's
//! buffer; the parser's own copy is dropped.

use std::borrow::Cow;
use std::sync::Arc;

use super::tokenizer::{Kind as TK, Token, Tokenizer};
use super::Type;
use crate::err;
use crate::kit::ast::{Comment, ExtKind, Extended, KeyValue, KindTag, NodeId, NodeKind, Number, NumberKind, Style, Tag, TagDirective};
use crate::kit::document::Document;
use crate::kit::parse_diagnostic;
use crate::kit::row_writer::RowWriter;
use crate::kit::span::Span;
use crate::kit::table::{self, Table};
use crate::kit::yaml_schema::{classify1_1_number, classify_number, core_tag_name, has_radix_prefix, NumberClass};
use crate::kit::{datetime, eql_any, unicode};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerKind {
    Sequence,
    Mapping,
}

#[derive(Debug, Clone, Copy)]
struct OpenContainer<'s> {
    id: NodeId,
    kind: ContainerKind,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    pending_key: Option<NodeId>,
    pending_value_span: usize,
    /// A comment on the `key:` line whose value is a block collection on the
    /// following lines (`contents: # note\n- a`). It belongs to the entry, not to
    /// the value's first child, so it is parked here and bound to the value (as
    /// its trailing comment) when the entry is completed in `finish_value`.
    pending_value_trailing: Option<Comment<'s>>,
    pending_sequence_item_span: Option<usize>,
    pending_sequence_item: bool,
    /// The `-` that introduced the item currently being parsed into this
    /// sequence, recorded against the item's node when `finish_value` attaches
    /// it (`Document::marker_span`).
    current_marker: Option<Span>,
    /// The `:` value indicator of the entry being parsed into this mapping,
    /// recorded against the `keyvalue` node when `finish_value` builds it
    /// (`Document::sep_span`). `None` for an explicit key that took no
    /// indicator.
    pending_sep_span: Option<Span>,
    continues_sequence_item: bool,
    /// True for an indentless block sequence value: one opened (with no fresh
    /// indent of its own) directly inside a mapping, so it shares that mapping's
    /// column (`k:\n  revs:\n  - 1`). The single dedent that matches that shared
    /// column must close BOTH this sequence and its enclosing mapping.
    shares_parent_indent: bool,
    /// True between an explicit key (`? key`) and its value indicator (`:`).
    explicit_awaiting_value: bool,
    /// True while a complex explicit key (a block sequence/mapping written after
    /// `?`) is being parsed: the next child container that closes into this
    /// mapping is the key, not a value.
    building_explicit_key: bool,
    /// Source column of the `?` indicator while `building_explicit_key`.
    explicit_key_col: usize,
}

impl<'s> OpenContainer<'s> {
    fn new(id: NodeId, kind: ContainerKind) -> Self {
        OpenContainer {
            id,
            kind,
            first_child: None,
            last_child: None,
            pending_key: None,
            pending_value_span: 0,
            pending_value_trailing: None,
            pending_sequence_item_span: None,
            pending_sequence_item: false,
            current_marker: None,
            pending_sep_span: None,
            continues_sequence_item: false,
            shares_parent_indent: false,
            explicit_awaiting_value: false,
            building_explicit_key: false,
            explicit_key_col: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingTag<'s> {
    text: &'s [u8],
    span: Span,
}

#[derive(Debug, Clone, Copy)]
struct TrailingComment<'s> {
    id: NodeId,
    comment: Comment<'s>,
}

#[derive(Debug, Clone, Copy)]
struct PendingAnchor<'s> {
    name: &'s [u8],
    span: Span,
}

#[derive(Debug, Clone, Copy)]
struct AnchorDef<'s> {
    name: &'s [u8],
    node: NodeId,
}

/// What a diagnostic says, one level finer than the error. The error a parse
/// RETURNS is part of the library's contract (the C ABI maps it to a status,
/// the conformance suite pins it), so a finer account of the same failure —
/// the flow collection that never closed, rather than the `UnexpectedToken`
/// the parser returns for it — lives here instead, beside the offset. Each
/// error has a code of its own (`code_for`); the rest are refinements a
/// failure site picks when it knows more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    UnexpectedToken,
    /// A `[` with no `]` before the end of the document. Points at the `[`.
    UnclosedFlowSequence,
    /// A `{` with no `}` before the end of the document. Points at the `{`.
    UnclosedFlowMapping,
    /// Something other than `,` or `]` after a flow sequence item.
    FlowSequenceSeparator,
    /// Something other than `,` or `}` after a flow mapping entry.
    FlowMappingSeparator,
    /// A `]`/`}` with no open flow collection to close.
    UnmatchedFlowClose,
    /// A `key: value` whose value itself reads as a `key: value` on the same line.
    NestedMappingOnLine,
    /// A block collection begun on the `---` line.
    BlockOnDocumentStartLine,
    /// A block collection with no key to be the value of.
    ContinuationAfterValue,
    /// Content after an inline value on the same line.
    TrailingContent,
    /// A value where no key or item is waiting for one.
    StrayValue,
    /// A plain scalar in flow that begins with a block indicator or `#`.
    IndicatorInFlow,
    /// A continuation line inside `[...]`/`{...}` not indented past the
    /// line the collection opened on.
    FlowIndent,
    /// A `key: - item` — a block sequence begun on its key's line.
    SequenceOnKeyLine,
    /// A `key:` at the column of a sequence's `- ` items, with no mapping
    /// around the sequence to return to.
    KeyBesideSequence,
    EmptyDocument,
    UnclosedString,
    InvalidEscape,
    InvalidUnicodeEscape,
    MultipleDocuments,
    DuplicateProperty,
    UndefinedAlias,
    InvalidDirective,
    /// A directive with no `---` after it.
    MissingDocumentStart,
    UndefinedTagHandle,
    InvalidIndent,
    TabIndent,
    InvalidBlockHeader,
    InvalidTag,
    InvalidAnchor,
    InvalidAlias,
    OutOfMemory,
}

/// The code a failure gets when its site said nothing finer.
pub fn code_for(e: Error) -> Code {
    match e.name() {
        "UnexpectedToken" => Code::UnexpectedToken,
        "EmptyDocument" => Code::EmptyDocument,
        "UnclosedString" => Code::UnclosedString,
        "InvalidUnicodeEscape" => Code::InvalidUnicodeEscape,
        "MultipleDocuments" => Code::MultipleDocuments,
        "DuplicateProperty" => Code::DuplicateProperty,
        "UndefinedAlias" => Code::UndefinedAlias,
        "InvalidDirective" => Code::InvalidDirective,
        "UndefinedTagHandle" => Code::UndefinedTagHandle,
        "InvalidIndent" => Code::InvalidIndent,
        "TabIndent" => Code::TabIndent,
        "InvalidBlockHeader" => Code::InvalidBlockHeader,
        "InvalidTag" => Code::InvalidTag,
        "InvalidAnchor" => Code::InvalidAnchor,
        "InvalidAlias" => Code::InvalidAlias,
        _ => Code::OutOfMemory,
    }
}

/// The teaching message for `code` — one sentence naming the fix, the same
/// contract as the other languages' `describe` (DESIGN.md "every diagnostic
/// names the fix").
pub fn describe(code: Code) -> &'static str {
    match code {
        Code::UnexpectedToken => "YAML cannot fit this into the structure around it; check this line's indentation against the lines above, and for a missing `:` after a key or `- ` before an item",
        Code::UnclosedFlowSequence => "this `[` is never closed; add the matching `]` (a flow sequence may span lines, but must end before the document does)",
        Code::UnclosedFlowMapping => "this `{` is never closed; add the matching `}` (a flow mapping may span lines, but must end before the document does)",
        Code::FlowSequenceSeparator => "expected `,` or `]` after this flow sequence item; separate items with commas, and quote an item that contains `[`, `]`, `{`, `}`, `,` or a leading `- `",
        Code::FlowMappingSeparator => "expected `,` or `}` after this flow mapping entry; separate entries with commas, and quote a key or value that contains `[`, `]`, `{`, `}`, `,` or a leading `- `",
        Code::UnmatchedFlowClose => "this closing bracket has no opening `[`/`{` to match; remove it, or quote the value if the bracket is meant as text",
        Code::NestedMappingOnLine => "a mapping cannot start on the same line as another key's value; put the nested `key: value` on its own, more-indented line, or quote the value if the `: ` is text",
        Code::BlockOnDocumentStartLine => "a block mapping or sequence cannot start on the `---` line; move it to the next line",
        Code::ContinuationAfterValue => "this is indented under an entry that already has a value, so it has no key to belong to; dedent it to be a sibling, or move the entry's value onto its own lines below it",
        Code::TrailingContent => "unexpected content after this line's value; a value ends its line — put the next entry on a line of its own, or quote the value if this is part of it",
        Code::StrayValue => "this value belongs to no key or item; indent it under the key it is the value of, or add a `:` if it is meant to be a key",
        Code::IndicatorInFlow => "a plain value inside `[...]`/`{...}` cannot start with `- `, `? `, `: ` or `#` — flow items are separated by commas, not dashes; drop the indicator, or quote the value",
        Code::FlowIndent => "this line is still inside a `[`/`{` opened above — either that bracket was never closed, or this line continues it and must be indented past the line it opened on (only the closing bracket may sit at that column)",
        Code::SequenceOnKeyLine => "a block sequence cannot start on the same line as its key; put `- item` on the next line, indented under the key (or use a flow sequence `[a, b]`), or quote the value if the dash is text",
        Code::KeyBesideSequence => "a `key:` cannot sit at the same indentation as a sequence's `- ` items; make every entry at this level a `- item` or every one a `key: value`, or indent the key under an item",
        Code::EmptyDocument => "the document is empty",
        Code::UnclosedString => "unclosed quoted string; add the closing quote, and escape any quote of the same kind inside it (`\\\"` in double quotes, `''` in single)",
        Code::InvalidEscape => "invalid escape in a double-quoted string; YAML allows \\0 \\a \\b \\t \\n \\v \\f \\r \\e \\\" \\/ \\\\ \\N \\_ \\L \\P \\xXX \\uXXXX \\UXXXXXXXX — use single quotes for a literal backslash",
        Code::InvalidUnicodeEscape => "invalid unicode escape; the hex digits after `\\x`/`\\u`/`\\U` must name a valid codepoint (and not a lone surrogate)",
        Code::MultipleDocuments => "a second document in the stream; fig reads one YAML document per file — remove the extra `---`/`...` or split the file",
        Code::DuplicateProperty => "this node already has an anchor or tag of this kind; a node takes at most one `&anchor` and one `!tag`",
        Code::UndefinedAlias => "this alias names no anchor defined before it; define `&name` on an earlier node, or check the spelling",
        Code::InvalidDirective => "malformed or misplaced directive; `%YAML` takes one `major.minor` version (once per document), `%TAG` a handle and a prefix, and both must come before the `---`",
        Code::MissingDocumentStart => "a directive must be followed by a `---` line before the document's content",
        Code::UndefinedTagHandle => "this tag uses a named handle (`!name!`) that no `%TAG` directive declares; declare it above the `---`, or use `!` or `!!`",
        Code::InvalidIndent => "inconsistent indentation; this line's indent matches no enclosing block — align it with its siblings, or indent it further under a key that ends with `:`",
        Code::TabIndent => "a tab cannot indent YAML structure; indent with spaces",
        Code::InvalidBlockHeader => "invalid block scalar header; `|` or `>` takes at most one indentation digit (1-9) and one chomping indicator (`-` or `+`)",
        Code::InvalidTag => "malformed tag; a tag is `!name`, `!!type`, `!handle!name` or `!<uri>`, followed by a space before its value",
        Code::InvalidAnchor => "malformed anchor; `&` must be followed by a name and a space before its value",
        Code::InvalidAlias => "malformed alias; `*` must be followed by the name of an anchor — quote the value if the `*` is text",
        Code::OutOfMemory => "out of memory",
    }
}

/// A few words for the caret annotation.
pub fn short_label(code: Code) -> &'static str {
    match code {
        Code::UnexpectedToken => "unexpected token",
        Code::UnclosedFlowSequence => "unclosed `[`",
        Code::UnclosedFlowMapping => "unclosed `{`",
        Code::FlowSequenceSeparator => "expected `,` or `]`",
        Code::FlowMappingSeparator => "expected `,` or `}`",
        Code::UnmatchedFlowClose => "unmatched close",
        Code::NestedMappingOnLine => "nested mapping on one line",
        Code::BlockOnDocumentStartLine => "block collection on `---` line",
        Code::ContinuationAfterValue => "no key for this",
        Code::TrailingContent => "trailing content",
        Code::StrayValue => "value with no key",
        Code::IndicatorInFlow => "block indicator in flow",
        Code::FlowIndent => "still inside `[`/`{`",
        Code::SequenceOnKeyLine => "sequence on key line",
        Code::KeyBesideSequence => "key among sequence items",
        Code::EmptyDocument => "empty document",
        Code::UnclosedString => "unclosed string",
        Code::InvalidEscape => "invalid escape",
        Code::InvalidUnicodeEscape => "invalid unicode escape",
        Code::MultipleDocuments => "second document",
        Code::DuplicateProperty => "duplicate property",
        Code::UndefinedAlias => "undefined alias",
        Code::InvalidDirective => "invalid directive",
        Code::MissingDocumentStart => "missing `---`",
        Code::UndefinedTagHandle => "undeclared tag handle",
        Code::InvalidIndent => "bad indentation",
        Code::TabIndent => "tab indentation",
        Code::InvalidBlockHeader => "invalid block header",
        Code::InvalidTag => "invalid tag",
        Code::InvalidAnchor => "invalid anchor",
        Code::InvalidAlias => "invalid alias",
        Code::OutOfMemory => "out of memory",
    }
}

/// A parse failure and the byte span it points at — the shape every other
/// language's `Diagnostic` has, so the CLI renders a YAML failure through the
/// same `file:line:col` report (see `parse_diagnostic`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    pub offset: usize,
    pub end: Option<usize>,
}

impl Diagnostic {
    /// 1-based line/column of `offset`, plus the full offending line.
    pub fn locate<'a>(&self, source: &'a [u8]) -> parse_diagnostic::Location<'a> {
        parse_diagnostic::locate_offset(source, self.offset)
    }

    /// Render `file:line:col: error: <message>` + source line + caret.
    pub fn render_alloc(&self, source: &[u8], file: &[u8]) -> Vec<u8> {
        parse_diagnostic::render_report_alloc(source, self.offset, file, b"error", describe(self.code).as_bytes())
    }
}

/// Everything a parse reports besides the tree. A lone `diag`: the parser
/// stops at its first error and has no authoring lints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub diag: Option<Diagnostic>,
}

/// `parse`, but on failure also fills `out.diag` with where the parse failed
/// and why. The error returned is the one `parse` returns.
pub fn parse_with_report(input: &[u8], format: Type, out: &mut Report) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_with_report_into(&mut t, format, out)?;
    adopt(source, &mut t)
}

/// `input` as an `AST`, alone.
pub fn parse_abstract(input: &[u8], format: Type) -> Result<crate::kit::ast::AST> {
    Ok(parse(input, format)?.ast)
}

/// `input` as a `Document`: its table written and adopted, with YAML's
/// `<<` merge key.
pub fn parse(input: &[u8], format: Type) -> Result<Document> {
    let source: Arc<[u8]> = Arc::from(input);
    let mut t = Table::init(source.clone());
    parse_into(&mut t, format)?;
    adopt(source, &mut t)
}

/// Write the table's source's rows into `t`, the table fig hands a parse.
pub fn parse_into(t: &mut Table, format: Type) -> Result<()> {
    let src = t.source_arc();
    let mut parser = Parser::new(&src, RowWriter::init(t));
    parser.parse_once(format)
}

/// `parse_into`, but on failure also fills `out.diag` with where the parse
/// failed and why, as `parse_with_report` does.
pub fn parse_with_report_into(t: &mut Table, format: Type, out: &mut Report) -> Result<()> {
    let src = t.source_arc();
    let mut parser = Parser::new(&src, RowWriter::init(t));
    let r = parser.parse_once(format);
    if r.is_err() {
        out.diag = parser.failure;
    }
    r
}

/// The finished table `t` as a `Document`, with YAML's merge key, as fig
/// would read it through the language's syntax.
fn adopt(source: Arc<[u8]>, t: &mut Table) -> Result<Document> {
    let mut doc = RowWriter::adopt(source, t)?;
    doc.ast.merge_key = super::syntax(Type::V1_2_2).merge_key;
    Ok(doc)
}

/// A scalar's kind as `scalar_kind` reads it: a string, maybe decoded into a
/// copy, or any other kind over the source.
enum ScalarKind<'s> {
    String(Cow<'s, [u8]>),
    Other(NodeKind<'s>),
}

pub struct Parser<'s, 't> {
    /// The table being written. A node's id is its row until `finish` puts
    /// the rows in pre-order, so ids are in the order the parser met the nodes.
    w: RowWriter<'t>,
    /// Every anchor defined, in node-id order: the name→id table the alias
    /// check reads (`resolve_aliases_or_error`). Names borrow `source`.
    anchors: Vec<AnchorDef<'s>>,
    /// Node properties (`!tag`, `&anchor`) seen but not yet attached to the node
    /// they decorate. Consumed by the next `add_node`.
    pending_tag: Option<PendingTag<'s>>,
    pending_anchor: Option<PendingAnchor<'s>>,
    /// When a second property of the same kind appears before its node is built,
    /// the earlier one belongs to a container that opens first and its first
    /// key/element carries the later one (`&map\n&key k: v`). The earlier
    /// property is parked here and claimed by the next `open_container`.
    container_tag: Option<PendingTag<'s>>,
    container_anchor: Option<PendingAnchor<'s>>,
    container_stack: Vec<OpenContainer<'s>>,
    /// Tag handles declared by `%TAG` directives in this document, with the
    /// prefix each expands to. Each is also written into the table
    /// (`RowWriter::directive`); the strings alias `source`.
    tag_directives: Vec<TagDirective<'s>>,
    tokens: Vec<Token>,
    index: usize,
    /// Comment layer. Captured centrally in `advance` (the only place a comment
    /// token is consumed) and reset on each newline. `pending_leading` buffers
    /// own-line comments until the next non-container node claims them.
    pending_leading: Vec<Comment<'s>>,
    /// Trailing comments in the order they were bound, written into the table
    /// when the document is done; a later one on the same node replaces an
    /// earlier.
    trailing: Vec<TrailingComment<'s>>,
    last_value_id: Option<NodeId>,
    /// True between a value-indicator `:` and the line's end.
    colon_line: bool,
    /// Suppresses leading-comment claiming while a container node is built.
    parking_container: bool,
    force_new_container: bool,
    /// Count of indents opened solely to hold a scalar/flow value on the line
    /// after its key (`key:\n  value`).
    value_only_indents: usize,
    root: Option<NodeId>,
    doc_started: bool,
    doc_ended: bool,
    /// True while still on the `---` marker line.
    on_doc_start_line: bool,
    /// True once a directive has been seen for the current document but its
    /// `---` directives-end marker has not yet appeared.
    directives_pending: bool,
    /// True once a `%YAML` directive has been seen for the current document.
    yaml_directive_seen: bool,

    source: &'s [u8],
    /// The YAML version whose scalar-resolution rules `scalar_kind` applies.
    version: Type,
    /// Where the parse failed and why, set at the moment it did (see
    /// `Diagnostic`).
    pub failure: Option<Diagnostic>,
}

/// A pending property taken off the parser while a mapping container opens, to
/// be put back for the key. See `hold_key_line_props`.
#[derive(Default)]
struct HeldProps<'s> {
    anchor: Option<PendingAnchor<'s>>,
    tag: Option<PendingTag<'s>>,
}

impl<'s, 't> Parser<'s, 't> {
    pub fn new(source: &'s [u8], w: RowWriter<'t>) -> Self {
        Parser {
            w,
            anchors: Vec::new(),
            pending_tag: None,
            pending_anchor: None,
            container_tag: None,
            container_anchor: None,
            container_stack: Vec::new(),
            tag_directives: Vec::new(),
            tokens: Vec::new(),
            index: 0,
            pending_leading: Vec::new(),
            trailing: Vec::new(),
            last_value_id: None,
            colon_line: false,
            parking_container: false,
            force_new_container: false,
            value_only_indents: 0,
            root: None,
            doc_started: false,
            doc_ended: false,
            on_doc_start_line: false,
            directives_pending: false,
            yaml_directive_seen: false,
            source,
            version: Type::V1_2_2,
            failure: None,
        }
    }

    /// Write `source` into the table `self.w` writes.
    pub fn parse_once(&mut self, format: Type) -> Result<()> {
        self.version = format;
        self.failure = None;

        let input = self.source;
        let mut tokenizer = Tokenizer::with_type(input, format);
        match tokenizer.tokenize() {
            Ok(tokens) => self.tokens = tokens,
            Err(e) => {
                if let Some(f) = tokenizer.failure {
                    // Under-indentation inside `[...]`/`{...}` breaks a different
                    // rule from the block one, and has a different fix.
                    let code = if e.is("InvalidIndent") && tokenizer.flow_depth > 0 { Code::FlowIndent } else { code_for(e) };
                    self.failure = Some(Diagnostic { code, offset: f.offset, end: f.end });
                }
                return Err(e);
            }
        }

        let r = self.parse_tokens();
        if let Err(e) = r {
            // A site that knew better already said where; everything else is
            // pinned on the token the cursor rests on.
            if self.failure.is_none() && !e.is("OutOfMemory") {
                let at = self.tokens[self.index.min(self.tokens.len() - 1)];
                self.failure = Some(Diagnostic {
                    code: code_for(e),
                    offset: at.span.start,
                    end: if at.span.end > at.span.start { Some(at.span.end) } else { None },
                });
            }
            // A token that does not fit, on a line indented with a tab: the tab
            // is what to fix.
            if let Some(f) = self.failure {
                if matches!(f.code, Code::UnexpectedToken | Code::StrayValue) {
                    if let Some(tab) = leading_tab(input, f.offset) {
                        self.failure = Some(Diagnostic { code: Code::TabIndent, offset: tab, end: None });
                    }
                }
            }
            return Err(e);
        }
        Ok(())
    }

    /// Record that the parse failed at `span` for the reason `code`, and hand
    /// back `e` to return.
    fn fail_at(&mut self, e: Error, code: Code, span: Span) -> Error {
        self.failure = Some(Diagnostic { code, offset: span.start, end: if span.end > span.start { Some(span.end) } else { None } });
        e
    }

    /// `fail_at` on the token the cursor rests on.
    fn fail_here(&mut self, e: Error, code: Code) -> Error {
        let span = self.peek().span;
        self.fail_at(e, code, span)
    }

    /// The byte offset of `slice` in the source, when it is a view into it.
    fn source_offset_of(&self, slice: &[u8]) -> Option<usize> {
        let base = self.source.as_ptr() as usize;
        let at = slice.as_ptr() as usize;
        if at < base || at > base + self.source.len() {
            return None;
        }
        Some(at - base)
    }

    /// The statement loop and document assembly, over the tokens `parse_once`
    /// produced.
    fn parse_tokens(&mut self) -> Result<()> {
        self.pending_leading.reserve(self.tokens.len());
        self.trailing.reserve(self.tokens.len());

        loop {
            self.skip_trivia_no_newline();
            // A next-line scalar value occupies a fresh indent by itself; the only
            // thing that may follow it at that indent is its closing dedent.
            if self.value_only_indents > 0 {
                match self.peek().kind {
                    TK::Newline | TK::Dedent | TK::EndOfFile => {}
                    _ => return Err(err!(UnexpectedToken)),
                }
            }
            // A directive must be followed by a `---` directives-end marker.
            if self.directives_pending {
                match self.peek().kind {
                    TK::Directive | TK::Newline | TK::DocStart => {}
                    _ => return Err(self.fail_here(err!(InvalidDirective), Code::MissingDocumentStart)),
                }
            }
            match self.peek().kind {
                TK::Indent => {
                    if !self.container_stack.is_empty() && self.cur().continues_sequence_item {
                        // A compact item container (`- key:` / `- -`) shares its
                        // sequence's dash level and opened no indent of its own. An
                        // indent landing AT the item's own column is its content;
                        // one landing DEEPER than the innermost is the pending key's
                        // block value: open a fresh container for it.
                        let col = self.column_of(self.peek().span.end);
                        if col > self.current_container_indent() {
                            self.force_new_container = true;
                        } else {
                            // A column between two nested compact items begins no node.
                            if !self.land_continuations(col)? {
                                return Err(self.fail_here(err!(UnexpectedToken), Code::InvalidIndent));
                            }
                        }
                    } else {
                        // An entry whose node is already whole takes nothing more
                        // indented.
                        if !self.container_stack.is_empty() {
                            let c = self.cur();
                            let whole = match c.kind {
                                ContainerKind::Sequence => !c.pending_sequence_item,
                                ContainerKind::Mapping => c.explicit_awaiting_value,
                            };
                            if whole {
                                self.advance();
                                return Err(self.fail_here(err!(UnexpectedToken), Code::ContinuationAfterValue));
                            }
                        }
                        self.force_new_container = true;
                    }
                    self.advance();
                }
                TK::Dedent => {
                    // A dedent matching an indent that only held a next-line scalar
                    // value closes no container — just consume it.
                    if self.value_only_indents > 0 {
                        self.value_only_indents -= 1;
                        self.advance();
                    } else if self.force_new_container && (self.pending_anchor.is_some() || self.pending_tag.is_some()) {
                        // The indent being closed held only a node property: it
                        // opened no container, so skip the close and keep the
                        // property for the value at the dedented level.
                        self.force_new_container = false;
                        self.advance();
                    } else {
                        self.close_pending_empty_value()?;
                        let dedent = self.advance();
                        let dedent_col = self.column_of(dedent.span.start);
                        // Close the container at this indent, and its parents
                        // that share its indent level.
                        loop {
                            let mut close_parent_too = !self.container_stack.is_empty()
                                && (self.cur().shares_parent_indent || self.cur().continues_sequence_item);
                            // A complex explicit key with no `:` value: this dedent
                            // must also close the explicit mapping with a null value
                            // — but only when it lands BELOW the `?` column.
                            if !close_parent_too && self.container_stack.len() >= 2 {
                                let parent = self.container_stack[self.container_stack.len() - 2];
                                if parent.building_explicit_key && dedent_col < parent.explicit_key_col {
                                    close_parent_too = true;
                                }
                            }
                            let id = self.close_container(dedent.span.end)?;
                            self.finish_value(id)?;
                            if !close_parent_too {
                                break;
                            }
                            self.close_pending_empty_value()?;
                        }
                        // A dedent landing exactly at an open compact item's own
                        // column returns to a SIBLING within that item.
                        self.land_continuations(dedent_col)?;
                    }
                }
                TK::Newline => {
                    self.on_doc_start_line = false;
                    self.advance();
                }
                TK::DocStart => {
                    // A second document start (or one after content) is multi-doc,
                    // which is out of scope.
                    if self.doc_started || self.w.count() > 0 {
                        return Err(err!(MultipleDocuments));
                    }
                    self.doc_started = true;
                    self.on_doc_start_line = true;
                    // The `---` ends any directives prefix and resolves it.
                    self.directives_pending = false;
                    self.advance();
                }
                TK::Directive => {
                    // Directives may only introduce a document: before its `---`,
                    // with no content yet.
                    if self.doc_started || self.doc_ended || self.root.is_some() || self.w.count() > 0 || !self.container_stack.is_empty() {
                        return Err(err!(InvalidDirective));
                    }
                    let tok = self.advance();
                    if let Err(e) = self.parse_directive(tok.source(self.source)) {
                        if e.is("OutOfMemory") {
                            return Err(e);
                        }
                        return Err(self.fail_at(e, code_for(e), tok.span));
                    }
                    self.directives_pending = true;
                }
                TK::DocEnd => {
                    self.doc_ended = true;
                    self.advance();
                }
                TK::Dash => {
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    // A block sequence cannot begin on the `---` marker line.
                    if self.on_doc_start_line {
                        return Err(self.fail_here(err!(UnexpectedToken), Code::BlockOnDocumentStartLine));
                    }
                    // A block sequence's `-` must begin its own line.
                    if self.pending_prop_on_line_of(self.peek().span.start) {
                        return Err(err!(UnexpectedToken));
                    }
                    self.close_sequence_item_continuation()?;
                    self.parse_sequence_entry()?;
                }
                TK::ExplicitKey => {
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    self.close_sequence_item_continuation()?;
                    self.parse_explicit_key()?;
                }
                TK::Colon => {
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    // A complex key whose sequence is still open on the same line
                    // as its `:` must be closed first.
                    self.close_open_complex_key()?;
                    // A deferred explicit key whose `:` arrives with no key
                    // content: the key is empty (null).
                    if !self.container_stack.is_empty() && self.cur().building_explicit_key {
                        let at = self.peek().span.start;
                        let null_key = self.add_node(NodeKind::Null, Span::new(at, at))?;
                        let m = self.cur_mut();
                        m.building_explicit_key = false;
                        m.pending_key = Some(null_key);
                        m.explicit_awaiting_value = true;
                    }
                    if !self.container_stack.is_empty() && self.cur().explicit_awaiting_value {
                        // A standalone `:` supplying the value for an explicit key.
                        let colon = self.advance();
                        let c = self.cur_mut();
                        c.explicit_awaiting_value = false;
                        c.pending_value_span = colon.span.end;
                        c.pending_sep_span = Some(colon.span);
                        // An explicit value (`: - one`) may take a compact block sequence.
                        self.parse_mapping_value(true)?;
                    } else {
                        // A leading `:` starts a block mapping entry with an empty key.
                        self.close_sequence_item_continuation()?;
                        self.parse_empty_key_entry()?;
                    }
                }
                TK::Scalar => {
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    self.close_sequence_item_continuation()?;
                    if self.is_mapping_start() {
                        // A block mapping cannot begin on the `---` marker line.
                        if self.on_doc_start_line {
                            return Err(self.fail_here(err!(UnexpectedToken), Code::BlockOnDocumentStartLine));
                        }
                        self.parse_mapping_entry()?;
                    } else if self.container_stack.is_empty() && self.root.is_none() {
                        // A bare scalar is a valid single-node document, but a plain
                        // scalar cannot begin with a flow indicator.
                        if invalid_plain_start(self.peek().source(self.source)) {
                            return Err(err!(UnexpectedToken));
                        }
                        let value_id = self.parse_scalar()?;
                        self.finish_value(value_id)?;
                    } else if self.force_new_container && self.current_awaits_value() {
                        // A plain scalar on the line after its key (`key:\n  value`).
                        let value_id = self.parse_scalar()?;
                        self.attach_deferred_value(value_id)?;
                    } else {
                        return Err(self.fail_here(err!(UnexpectedToken), Code::StrayValue));
                    }
                }
                TK::Alias => {
                    // An alias is a whole value node (`k: *a`, `- *a`, `*a : b`).
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    self.close_sequence_item_continuation()?;
                    if self.is_mapping_start() {
                        self.parse_mapping_entry()?;
                    } else if self.container_stack.is_empty() && self.root.is_none() {
                        let value_id = self.parse_alias()?;
                        self.finish_value(value_id)?;
                    } else if self.force_new_container && self.current_awaits_value() {
                        let value_id = self.parse_alias()?;
                        self.attach_deferred_value(value_id)?;
                    } else {
                        return Err(self.fail_here(err!(UnexpectedToken), Code::StrayValue));
                    }
                }
                TK::BlockHeader => {
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    self.close_sequence_item_continuation()?;
                    if self.container_stack.is_empty() && self.root.is_none() {
                        let value_id = self.parse_block_scalar()?;
                        self.finish_value(value_id)?;
                    } else if self.force_new_container && self.current_awaits_value() {
                        // A block scalar written on the line after its key (`k:\n  >1`).
                        let value_id = self.parse_block_scalar()?;
                        self.attach_deferred_value(value_id)?;
                    } else {
                        return Err(self.fail_here(err!(UnexpectedToken), Code::StrayValue));
                    }
                }
                TK::FlowSeqStart | TK::FlowMapStart => {
                    // A flow collection as a whole document, the value of a mapping
                    // key / sequence item on the following line, or — when a `:`
                    // follows it — a block mapping key (`[a, b]: value`).
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    self.close_sequence_item_continuation()?;
                    let node_id = self.parse_flow_node()?;
                    self.skip_trivia_no_newline();
                    if self.peek().kind == TK::Colon {
                        // An implicit key must be a single line.
                        let span = self.w.span_of(node_id);
                        if self.source[span.start..span.end].contains(&b'\n') {
                            return Err(err!(UnexpectedToken));
                        }
                        let mapping_id = self.ensure_container(ContainerKind::Mapping)?;
                        self.close_pending_empty_value()?;
                        let colon = self.advance();
                        let parent = self.container_by_id(mapping_id);
                        parent.pending_key = Some(node_id);
                        parent.pending_value_span = colon.span.end;
                        parent.pending_sep_span = Some(colon.span);
                        self.parse_mapping_value(false)?;
                    } else {
                        self.attach_deferred_value(node_id)?;
                    }
                }
                TK::Tag | TK::Anchor => {
                    // A node property at a value/root position: stash it; the node
                    // it decorates is parsed on a following loop iteration.
                    if self.doc_ended {
                        return Err(err!(MultipleDocuments));
                    }
                    let in_container = !self.container_stack.is_empty();
                    self.consume_pending_properties()?;
                    // A property standing alone on its line inside a container is
                    // valid only as a deferred value that out-indents its key/dash.
                    if in_container {
                        if let TK::Newline | TK::Dedent | TK::EndOfFile = self.peek().kind {
                            if !(self.force_new_container && self.current_awaits_value()) {
                                return Err(err!(UnexpectedToken));
                            }
                        }
                    }
                }
                TK::EndOfFile => break,
                TK::FlowSeqEnd | TK::FlowMapEnd => return Err(self.fail_here(err!(UnexpectedToken), Code::UnmatchedFlowClose)),
                _ => return Err(err!(UnexpectedToken)),
            }
        }

        while !self.container_stack.is_empty() {
            self.close_pending_empty_value()?;
            let id = self.close_container(self.peek().span.end)?;
            self.finish_value(id)?;
        }

        // A parked container property that never opened a container means two
        // properties of one kind decorated a single node (`&a &b scalar`).
        if let Some(p) = self.container_anchor {
            return Err(self.fail_at(err!(DuplicateProperty), Code::DuplicateProperty, p.span));
        }
        if let Some(p) = self.container_tag {
            return Err(self.fail_at(err!(DuplicateProperty), Code::DuplicateProperty, p.span));
        }

        // Every alias must reference an anchor defined earlier in the document.
        self.resolve_aliases_or_error()?;

        // An empty document is valid; its root is the null node.
        let root = match self.root {
            Some(r) => r,
            None => {
                let span = self.peek().span;
                self.add_node(NodeKind::Null, span)?
            }
        };
        // End-of-document orphan comments dangle off the root.
        self.claim_dangling(root)?;
        let trailing = std::mem::take(&mut self.trailing);
        for t in &trailing {
            self.w.comment(t.id, table::COMMENT_TRAILING, t.comment)?;
        }
        self.w.finish(root)
    }

    /// Validate a directive line (`line` is the token text, starting with `%`).
    /// `%YAML` carries exactly one `major.minor` version and at most one may
    /// appear per document; `%TAG` carries a handle and a prefix; any other name
    /// is a reserved directive, accepted and ignored.
    fn parse_directive(&mut self, line: &'s [u8]) -> Result<()> {
        debug_assert!(!line.is_empty() && line[0] == b'%');
        let mut i: usize = 1;
        let name_start = i;
        while i < line.len() && !is_directive_space(line[i]) {
            i += 1;
        }
        let name = &line[name_start..i];
        if name.is_empty() {
            return Err(err!(InvalidDirective)); // `%` with no name
        }

        if name == b"YAML" {
            // At most one `%YAML` per document.
            if self.yaml_directive_seen {
                return Err(err!(InvalidDirective));
            }
            self.yaml_directive_seen = true;
            i = skip_directive_space(line, i);
            let ver_start = i;
            while i < line.len() && !is_directive_space(line[i]) {
                i += 1;
            }
            if !is_yaml_version(&line[ver_start..i]) {
                return Err(err!(InvalidDirective));
            }
            require_directive_end(line, i)?;
        } else if name == b"TAG" {
            i = skip_directive_space(line, i);
            let handle_start = i;
            while i < line.len() && !is_directive_space(line[i]) {
                i += 1;
            }
            let handle = &line[handle_start..i];
            if handle.is_empty() {
                return Err(err!(InvalidDirective)); // missing handle
            }
            i = skip_directive_space(line, i);
            let prefix_start = i;
            while i < line.len() && !is_directive_space(line[i]) {
                i += 1;
            }
            if i == prefix_start {
                return Err(err!(InvalidDirective)); // missing prefix
            }
            require_directive_end(line, i)?;
            // Record the handle and its prefix, so a named handle (`!e!`) may be
            // used by a tag and so the printer can re-emit the directive.
            self.tag_directives.push(TagDirective { handle, prefix: &line[prefix_start..i] });
            self.w.directive(handle, &line[prefix_start..i])?;
        }
        // else: a reserved directive — accepted and ignored.
        Ok(())
    }

    fn parse_sequence_entry(&mut self) -> Result<()> {
        let dash = self.advance();
        let sequence_id = self.ensure_container(ContainerKind::Sequence)?;
        // A preceding dash at this same indent with no value (`-\n- x`) is a null
        // entry: finalize it before starting this entry.
        if self.cur().id == sequence_id {
            self.close_pending_empty_value()?;
        }
        self.clear_pending_sequence_item(sequence_id);
        self.container_by_id(sequence_id).current_marker = Some(dash.span);
        self.skip_trivia_no_newline();
        self.consume_pending_properties()?; // `- !!str x`

        match self.peek().kind {
            TK::Newline | TK::Dedent | TK::EndOfFile => {
                let c = self.cur_mut();
                c.pending_sequence_item = true;
                c.pending_sequence_item_span = Some(dash.span.end);
            }
            TK::Scalar => {
                if self.is_mapping_start() {
                    // `- &a a: b`: the property is the key's, not the item
                    // mapping's (see `hold_key_line_props`).
                    let held = self.hold_key_line_props();
                    let mapping_id = self.open_container(ContainerKind::Mapping, self.peek().span.start)?;
                    self.container_by_id(mapping_id).continues_sequence_item = true;
                    self.release_key_line_props(held);
                    self.parse_mapping_entry()?;
                } else {
                    let value_id = self.parse_scalar()?;
                    self.finish_value(value_id)?;
                }
            }
            TK::BlockHeader => {
                let value_id = self.parse_block_scalar()?;
                self.finish_value(value_id)?;
            }
            TK::Alias => {
                // A sequence entry that is an alias (`- *a`), or an alias key
                // mapping entry (`- *a : b`).
                if self.is_mapping_start() {
                    let mapping_id = self.open_container(ContainerKind::Mapping, self.peek().span.start)?;
                    self.container_by_id(mapping_id).continues_sequence_item = true;
                    self.parse_mapping_entry()?;
                } else {
                    let value_id = self.parse_alias()?;
                    self.finish_value(value_id)?;
                }
            }
            TK::FlowSeqStart | TK::FlowMapStart => {
                let value_id = self.parse_flow_node()?;
                self.finish_value(value_id)?;
            }
            TK::ExplicitKey => {
                // A sequence entry whose value is an explicit-key mapping (`- ? k`).
                let mapping_id = self.open_container(ContainerKind::Mapping, self.peek().span.start)?;
                self.container_by_id(mapping_id).continues_sequence_item = true;
                self.parse_explicit_key()?;
            }
            TK::Colon => {
                // A sequence entry whose value is an empty-key mapping (`- : v`).
                let mapping_id = self.open_container(ContainerKind::Mapping, self.peek().span.start)?;
                self.container_by_id(mapping_id).continues_sequence_item = true;
                self.parse_empty_key_entry()?;
            }
            TK::Dash => {
                // A compact nested sequence (`- - c`): the inner dash opens a
                // sequence nested inside this one, rather than continuing it.
                let inner_id = self.open_container(ContainerKind::Sequence, self.peek().span.start)?;
                self.container_by_id(inner_id).continues_sequence_item = true;
                self.parse_sequence_entry()?;
            }
            _ => return Err(err!(UnexpectedToken)),
        }
        Ok(())
    }

    /// A property on an implicit key's own line decorates the KEY (`&a a: b`
    /// anchors the scalar `a`), while one on an earlier line decorates the
    /// mapping the key opens (`&m\na: b`). `open_container` hands a lone pending
    /// property to the container it opens, so a same-line one is held back here
    /// until the container exists, and `release_key_line_props` puts it back for
    /// the key's `add_node` to claim. Called with the key token next.
    fn hold_key_line_props(&mut self) -> HeldProps<'s> {
        if !self.pending_prop_on_line_of(self.peek().span.start) {
            return HeldProps::default();
        }
        HeldProps { anchor: self.pending_anchor.take(), tag: self.pending_tag.take() }
    }

    fn release_key_line_props(&mut self, held: HeldProps<'s>) {
        if let Some(a) = held.anchor {
            self.pending_anchor = Some(a);
        }
        if let Some(t) = held.tag {
            self.pending_tag = Some(t);
        }
    }

    fn parse_mapping_entry(&mut self) -> Result<()> {
        let held = self.hold_key_line_props();
        let mapping_id = self.ensure_container(ContainerKind::Mapping)?;
        self.close_pending_empty_value()?;
        self.release_key_line_props(held);

        let key_id = self.parse_key_node()?;
        self.skip_trivia_no_newline();
        if self.peek().kind != TK::Colon {
            return Err(err!(UnexpectedToken));
        }
        let colon = self.advance();

        {
            let parent = self.container_by_id(mapping_id);
            parent.pending_key = Some(key_id);
            parent.pending_value_span = colon.span.end;
            parent.pending_sep_span = Some(colon.span);
        }

        // An implicit key (`key:`) may not take a compact block sequence on its
        // own line; the sequence must start on the next line.
        self.parse_mapping_value(false)
    }

    /// Parses the value following a mapping key's `:` indicator. The pending key
    /// is already recorded on the current mapping container; an inline value is
    /// built and attached immediately, while a value written on following lines
    /// is left for the indent/dedent machinery to attach. `allow_compact` enables
    /// a block collection whose first entry sits on the `:` line, which is only
    /// legal after an explicit-value indicator.
    fn parse_mapping_value(&mut self, allow_compact: bool) -> Result<()> {
        self.skip_trivia_no_newline();
        self.consume_pending_properties()?; // `k: !!int 5` (or `k: !!str` → tagged null)
        match self.peek().kind {
            TK::Scalar => {
                if allow_compact && self.is_mapping_start() {
                    // A compact block mapping as an explicit value (`: moon: white`).
                    // A tab separating it from the `:` would act as the nested
                    // mapping's indentation, which is forbidden.
                    if self.tab_between(self.cur().pending_value_span, self.peek().span.start) {
                        return Err(err!(UnexpectedToken));
                    }
                    let child_id = self.open_container(ContainerKind::Mapping, self.peek().span.start)?;
                    self.parse_mapping_entry()?;
                    let end = self.w.span_of(child_id).end;
                    let id = self.close_container(end)?;
                    self.finish_value(id)?;
                } else {
                    // An inline value is a single scalar; it cannot itself be a
                    // block mapping, so a `:` trailing the value is junk.
                    let value_id = self.parse_scalar()?;
                    self.finish_value(value_id)?;
                    self.require_value_end()?;
                }
            }
            TK::Dash => {
                // A compact block sequence whose first entry sits on the `:` line
                // (explicit `: - one`). Only valid after an explicit-value indicator.
                if !allow_compact {
                    return Err(self.fail_here(err!(UnexpectedToken), Code::SequenceOnKeyLine));
                }
                let seq_id = self.open_container(ContainerKind::Sequence, self.peek().span.start)?;
                self.container_by_id(seq_id).continues_sequence_item = true;
                self.parse_sequence_entry()?;
            }
            TK::BlockHeader => {
                let value_id = self.parse_block_scalar()?;
                self.finish_value(value_id)?;
            }
            TK::Alias => {
                let value_id = self.parse_alias()?;
                self.finish_value(value_id)?;
                self.require_value_end()?;
            }
            TK::FlowSeqStart | TK::FlowMapStart => {
                let value_id = self.parse_flow_node()?;
                self.finish_value(value_id)?;
                self.require_value_end()?;
            }
            TK::Newline | TK::Dedent | TK::EndOfFile => {}
            _ => return Err(err!(UnexpectedToken)),
        }
        Ok(())
    }

    /// After an inline value, only same-line trivia and the line break may follow.
    fn require_value_end(&mut self) -> Result<()> {
        while self.peek().kind == TK::Whitespace {
            self.advance();
        }
        match self.peek().kind {
            TK::Newline | TK::Comment | TK::Dedent | TK::EndOfFile => Ok(()),
            TK::Colon => Err(self.fail_here(err!(UnexpectedToken), Code::NestedMappingOnLine)),
            TK::FlowSeqEnd | TK::FlowMapEnd => Err(self.fail_here(err!(UnexpectedToken), Code::UnmatchedFlowClose)),
            _ => Err(self.fail_here(err!(UnexpectedToken), Code::TrailingContent)),
        }
    }

    /// True when the source span `[from, to)` contains a tab.
    fn tab_between(&self, from: usize, to: usize) -> bool {
        if from > to || to > self.source.len() {
            return false;
        }
        self.source[from..to].contains(&b'\t')
    }

    /// Parses an explicit-key entry (`? key`). Records the key on the current
    /// mapping and marks it awaiting a `:` value indicator.
    fn parse_explicit_key(&mut self) -> Result<()> {
        let marker = self.advance(); // explicit_key `?`
        let mapping_id = self.ensure_container(ContainerKind::Mapping)?;
        self.close_pending_empty_value()?;

        self.skip_trivia_no_newline();
        self.consume_pending_properties()?; // `? !!str key`
        // `?` alone on its line: the key is supplied by following lines.
        if self.peek().kind == TK::Newline {
            let col = self.column_of(marker.span.start);
            let m = self.container_by_id(mapping_id);
            m.building_explicit_key = true;
            m.explicit_key_col = col;
            return Ok(());
        }
        let key_id = match self.peek().kind {
            TK::Scalar => {
                if self.is_mapping_start() {
                    // A complex key that is itself a block mapping (`? earth: blue`).
                    // A tab separating it from `?` acts as its indentation.
                    if self.tab_between(marker.span.end, self.peek().span.start) {
                        return Err(err!(UnexpectedToken));
                    }
                    let col = self.column_of(marker.span.start);
                    {
                        let m = self.container_by_id(mapping_id);
                        m.building_explicit_key = true;
                        m.explicit_key_col = col;
                    }
                    let key_map_id = self.open_container(ContainerKind::Mapping, self.peek().span.start)?;
                    self.container_by_id(key_map_id).continues_sequence_item = true;
                    self.parse_mapping_entry()?;
                    return Ok(());
                }
                self.parse_scalar()?
            }
            TK::FlowSeqStart | TK::FlowMapStart => {
                let node_id = self.parse_flow_node()?;
                self.skip_trivia_no_newline();
                if self.peek().kind == TK::Colon {
                    // A complex key that is itself a flow-keyed block mapping
                    // (`? []: x`).
                    let col = self.column_of(marker.span.start);
                    {
                        let m = self.container_by_id(mapping_id);
                        m.building_explicit_key = true;
                        m.explicit_key_col = col;
                    }
                    let start = self.w.span_of(node_id).start;
                    let key_map_id = self.open_container(ContainerKind::Mapping, start)?;
                    self.container_by_id(key_map_id).continues_sequence_item = true;
                    let colon = self.advance();
                    let km = self.container_by_id(key_map_id);
                    km.pending_key = Some(node_id);
                    km.pending_value_span = colon.span.end;
                    km.pending_sep_span = Some(colon.span);
                    self.parse_mapping_value(false)?;
                    return Ok(());
                }
                node_id
            }
            TK::BlockHeader => self.parse_block_scalar()?,
            TK::Alias => self.parse_alias()?, // `? *a`
            TK::Dash => {
                // A complex key that is itself a block sequence (`? - a\n  - b`).
                let col = self.column_of(marker.span.start);
                {
                    let m = self.container_by_id(mapping_id);
                    m.building_explicit_key = true;
                    m.explicit_key_col = col;
                }
                let seq_id = self.open_container(ContainerKind::Sequence, self.peek().span.start)?;
                self.container_by_id(seq_id).continues_sequence_item = true;
                self.parse_sequence_entry()?;
                return Ok(());
            }
            TK::Colon => {
                // A complex key that is itself an empty-key block mapping (`? : x`).
                let col = self.column_of(marker.span.start);
                {
                    let m = self.container_by_id(mapping_id);
                    m.building_explicit_key = true;
                    m.explicit_key_col = col;
                }
                let key_map_id = self.open_container(ContainerKind::Mapping, self.peek().span.start)?;
                self.container_by_id(key_map_id).continues_sequence_item = true;
                self.parse_empty_key_entry()?;
                return Ok(());
            }
            // `?` alone on its line with a decorating property, or `?` at end of
            // entry, is a null key.
            TK::Newline | TK::Dedent | TK::EndOfFile => self.add_node(NodeKind::Null, Span::new(marker.span.end, marker.span.end))?,
            _ => return Err(err!(UnexpectedToken)),
        };

        let end = self.w.span_of(key_id).end;
        let parent = self.container_by_id(mapping_id);
        parent.pending_key = Some(key_id);
        parent.pending_value_span = end;
        parent.pending_sep_span = None;
        parent.explicit_awaiting_value = true;
        Ok(())
    }

    /// Parses a block mapping entry whose key is empty (`: value`). The current
    /// token is the `:`; the key is the null node.
    fn parse_empty_key_entry(&mut self) -> Result<()> {
        let mapping_id = self.ensure_container(ContainerKind::Mapping)?;
        self.close_pending_empty_value()?;

        let colon = self.advance(); // `:`
        let key_id = self.add_node(NodeKind::Null, Span::new(colon.span.start, colon.span.start))?;
        {
            let parent = self.container_by_id(mapping_id);
            parent.pending_key = Some(key_id);
            parent.pending_value_span = colon.span.end;
            parent.pending_sep_span = Some(colon.span);
        }
        self.parse_mapping_value(false)
    }

    fn parse_scalar(&mut self) -> Result<NodeId> {
        if self.peek().kind != TK::Scalar {
            return Err(err!(UnexpectedToken));
        }
        let token = self.advance();
        // The decoders run on a token already consumed, so one that fails
        // without saying where is pinned on that token here.
        let kind = match self.scalar_kind(token.source(self.source)) {
            Ok(k) => k,
            Err(e) => {
                if self.failure.is_none() && !e.is("OutOfMemory") {
                    self.failure = Some(Diagnostic { code: code_for(e), offset: token.span.start, end: Some(token.span.end) });
                }
                return Err(e);
            }
        };
        match kind {
            ScalarKind::String(s) => self.add_node(NodeKind::String(&s), token.span),
            ScalarKind::Other(k) => self.add_node(k, token.span),
        }
    }

    /// Validates that every `*name` alias references an anchor `&name` defined
    /// earlier in the document (YAML aliases refer backward).
    fn resolve_aliases_or_error(&mut self) -> Result<()> {
        for i in 0..self.w.count() {
            let id = i as NodeId;
            let found = {
                let NodeKind::Alias(name) = self.w.kind_of(id) else { continue };
                let mut found = false;
                for a in &self.anchors {
                    if a.node >= id {
                        break; // sorted by id; no anchor at/after the alias
                    }
                    if a.name == name {
                        found = true;
                    }
                }
                found
            };
            if !found {
                let span = self.w.span_of(id);
                return Err(self.fail_at(err!(UndefinedAlias), Code::UndefinedAlias, span));
            }
        }
        Ok(())
    }

    /// Parses a `*name` alias token into an `alias` node.
    fn parse_alias(&mut self) -> Result<NodeId> {
        // An alias node takes no properties: `&b *a` / `!!str *a` are malformed.
        if self.pending_anchor.is_some() || self.pending_tag.is_some() {
            return Err(err!(UnexpectedToken));
        }
        let token = self.advance(); // .alias, span covers `*name`
        let name = &token.source(self.source)[1..];
        self.add_node(NodeKind::Alias(name), token.span)
    }

    /// Parses an implicit mapping key, which is usually a scalar but may also
    /// be an alias (`*a : b`).
    fn parse_key_node(&mut self) -> Result<NodeId> {
        if self.peek().kind == TK::Alias {
            self.parse_alias()
        } else {
            self.parse_scalar()
        }
    }

    /// Parses a block scalar: a `block_header` token, the header line's newline,
    /// then an optional `block_scalar` body token. Produces a single string node
    /// spanning the whole construct, with folding/chomping applied.
    fn parse_block_scalar(&mut self) -> Result<NodeId> {
        let header = self.advance();
        let header_source = header.source(self.source);

        // Skip a trailing comment and the header-line newline to reach the body.
        while let TK::Whitespace | TK::Comment = self.peek().kind {
            self.advance();
        }
        if self.peek().kind == TK::Newline {
            self.advance();
        }

        // An explicit indentation indicator counts from the owning node's column.
        let parent_indent = self.current_container_indent();

        let mut span_end = header.span.end;
        let mut value: Vec<u8> = Vec::new();
        if self.peek().kind == TK::BlockScalar {
            let body = self.advance();
            span_end = body.span.end;
            value = decode_block_scalar(header_source, parent_indent, body.source(self.source));
        }

        self.add_node(NodeKind::String(&value), Span::new(header.span.start, span_end))
    }

    /// The source column of the innermost open container. 0 at the document root.
    fn current_container_indent(&self) -> usize {
        let Some(c) = self.container_stack.last() else { return 0 };
        self.column_of(self.w.span_of(c.id).start)
    }

    /// The source column of byte position `pos`.
    fn column_of(&self, pos: usize) -> usize {
        crate::kit::lines::column_of(self.source, pos)
    }

    /// Parses a flow node: a flow sequence `[...]`, flow mapping `{...}`, or a
    /// flow scalar. Flow content ignores indentation and line breaks.
    fn parse_flow_node(&mut self) -> Result<NodeId> {
        self.skip_flow_trivia();
        loop {
            match self.peek().kind {
                TK::Tag => {
                    let t = self.advance();
                    self.stash_tag(t.source(self.source), t.span)?;
                }
                TK::Anchor => {
                    let a = self.advance();
                    self.stash_anchor(&a.source(self.source)[1..], a.span)?;
                }
                _ => break,
            }
            self.skip_flow_trivia();
        }
        // A property with no node of its own decorates an implicit null.
        if self.pending_tag.is_some() || self.pending_anchor.is_some() {
            if let TK::Comma | TK::Colon | TK::FlowMapEnd | TK::FlowSeqEnd = self.peek().kind {
                let at = self.peek().span.start;
                return self.add_node(NodeKind::Null, Span::new(at, at));
            }
        }
        match self.peek().kind {
            TK::FlowSeqStart => self.parse_flow_sequence(),
            TK::FlowMapStart => self.parse_flow_mapping(),
            TK::Alias => self.parse_alias(),
            TK::Scalar => {
                if invalid_flow_scalar(self.peek().source(self.source)) {
                    return Err(self.fail_here(err!(UnexpectedToken), Code::IndicatorInFlow));
                }
                self.parse_scalar()
            }
            _ => Err(err!(UnexpectedToken)),
        }
    }

    fn parse_flow_sequence(&mut self) -> Result<NodeId> {
        let open = self.advance(); // flow_seq_start
        let seq_id = self.add_node(NodeKind::Sequence(None), open.span)?;
        let mut last: Option<NodeId> = None;

        self.skip_flow_trivia();
        while self.peek().kind != TK::FlowSeqEnd {
            if self.peek().kind == TK::EndOfFile {
                return Err(self.fail_at(err!(UnexpectedToken), Code::UnclosedFlowSequence, open.span));
            }

            let item = self.parse_flow_sequence_item()?;
            if let Some(prev) = last {
                self.w.set_next(prev, Some(item));
            } else {
                self.w.set(seq_id, NodeKind::Sequence(Some(item)))?;
            }
            last = Some(item);

            self.skip_flow_trivia();
            match self.peek().kind {
                TK::Comma => {
                    self.advance();
                    self.skip_flow_trivia();
                }
                TK::FlowSeqEnd => {}
                TK::EndOfFile => return Err(self.fail_at(err!(UnexpectedToken), Code::UnclosedFlowSequence, open.span)),
                _ => return Err(self.fail_here(err!(UnexpectedToken), Code::FlowSequenceSeparator)),
            }
        }

        let close = self.advance(); // flow_seq_end
        self.set_end(seq_id, close.span.end);
        Ok(seq_id)
    }

    /// Parses a single flow-sequence element. Usually a plain flow node, but an
    /// element may also be a single-pair mapping: either explicit (`[? k : v]`)
    /// or implicit (`[a: b]`).
    fn parse_flow_sequence_item(&mut self) -> Result<NodeId> {
        let explicit = self.peek().kind == TK::ExplicitKey;
        if explicit {
            self.advance();
            self.skip_flow_trivia();
        }

        let key_id = match self.peek().kind {
            // An empty key: `[: v]` (value-only) or a bare `[?]`.
            TK::Colon => {
                let span = self.peek().span;
                self.add_node(NodeKind::Null, span)?
            }
            TK::Comma | TK::FlowSeqEnd => {
                if explicit {
                    let span = self.peek().span;
                    self.add_node(NodeKind::Null, span)?
                } else {
                    return Err(err!(UnexpectedToken));
                }
            }
            _ => self.parse_flow_node()?,
        };
        // An implicit key's `:` must be on the same line as the key.
        if explicit {
            self.skip_flow_trivia();
        } else {
            self.skip_flow_inline_trivia();
        }

        // Without a `:` it is a plain element — unless `?` already forced a pair.
        if self.peek().kind != TK::Colon {
            if !explicit {
                return Ok(key_id);
            }
            let at = self.w.span_of(key_id).end;
            let v = self.add_node(NodeKind::Null, Span::new(at, at))?;
            return self.wrap_flow_pair(key_id, v);
        }

        self.advance(); // colon
        self.skip_flow_trivia();
        let value_id = match self.peek().kind {
            TK::Comma | TK::FlowSeqEnd => {
                let at = self.w.span_of(key_id).end;
                self.add_node(NodeKind::Null, Span::new(at, at))?
            }
            _ => self.parse_flow_node()?,
        };
        self.wrap_flow_pair(key_id, value_id)
    }

    /// Wraps a key/value pair as a standalone single-entry mapping node.
    fn wrap_flow_pair(&mut self, key_id: NodeId, value_id: NodeId) -> Result<NodeId> {
        let key_span = self.w.span_of(key_id);
        let value_span = self.w.span_of(value_id);
        let pair_id = self.add_node(NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }), Span::new(key_span.start, value_span.end))?;
        self.add_node(NodeKind::Mapping(Some(pair_id)), Span::new(key_span.start, value_span.end))
    }

    fn parse_flow_mapping(&mut self) -> Result<NodeId> {
        let open = self.advance(); // flow_map_start
        let map_id = self.add_node(NodeKind::Mapping(None), open.span)?;
        let mut last: Option<NodeId> = None;

        self.skip_flow_trivia();
        while self.peek().kind != TK::FlowMapEnd {
            if self.peek().kind == TK::EndOfFile {
                return Err(self.fail_at(err!(UnexpectedToken), Code::UnclosedFlowMapping, open.span));
            }

            // An optional `?` introduces the key explicitly.
            if self.peek().kind == TK::ExplicitKey {
                self.advance();
                self.skip_flow_trivia();
            }

            let key_id = match self.peek().kind {
                // An empty key, e.g. `{: v}` (value-only) or a bare `{?}`.
                TK::Colon | TK::Comma | TK::FlowMapEnd => {
                    let span = self.peek().span;
                    self.add_node(NodeKind::Null, span)?
                }
                _ => self.parse_flow_node()?,
            };
            self.skip_flow_trivia();

            let value_id = if self.peek().kind == TK::Colon {
                self.advance();
                self.skip_flow_trivia();
                match self.peek().kind {
                    // `{a: , b}` / `{a:}` — an explicit but empty value is null.
                    TK::Comma | TK::FlowMapEnd => {
                        let at = self.w.span_of(key_id).end;
                        self.add_node(NodeKind::Null, Span::new(at, at))?
                    }
                    _ => self.parse_flow_node()?,
                }
            } else {
                // `{a, b}` — a bare key has an implicit null value.
                let at = self.w.span_of(key_id).end;
                self.add_node(NodeKind::Null, Span::new(at, at))?
            };

            let key_span = self.w.span_of(key_id);
            let value_span = self.w.span_of(value_id);
            let pair_id = self.add_node(NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }), Span::new(key_span.start, value_span.end))?;

            if let Some(prev) = last {
                self.w.set_next(prev, Some(pair_id));
            } else {
                self.w.set(map_id, NodeKind::Mapping(Some(pair_id)))?;
            }
            last = Some(pair_id);

            self.skip_flow_trivia();
            match self.peek().kind {
                TK::Comma => {
                    self.advance();
                    self.skip_flow_trivia();
                }
                TK::FlowMapEnd => {}
                TK::EndOfFile => return Err(self.fail_at(err!(UnexpectedToken), Code::UnclosedFlowMapping, open.span)),
                _ => return Err(self.fail_here(err!(UnexpectedToken), Code::FlowMappingSeparator)),
            }
        }

        let close = self.advance(); // flow_map_end
        self.set_end(map_id, close.span.end);
        Ok(map_id)
    }

    fn skip_flow_trivia(&mut self) {
        while let TK::Whitespace | TK::Newline | TK::Comment | TK::Indent | TK::Dedent = self.peek().kind {
            self.advance();
        }
    }

    /// Skips only same-line trivia (no line breaks).
    fn skip_flow_inline_trivia(&mut self) {
        while let TK::Whitespace | TK::Comment = self.peek().kind {
            self.advance();
        }
    }

    fn ensure_container(&mut self, kind: ContainerKind) -> Result<NodeId> {
        if !self.force_new_container && !self.container_stack.is_empty() {
            let current = *self.cur();
            if current.kind == kind {
                return Ok(current.id);
            }
            // A mapping key at a block sequence's indentation is valid only as
            // the return to an enclosing mapping after an indentless sequence
            // value (`one:\n- 2\nfour: 5`).
            if kind == ContainerKind::Mapping && current.kind == ContainerKind::Sequence {
                let n = self.container_stack.len();
                let has_enclosing_mapping = self.container_stack[..n - 1].iter().any(|c| c.kind == ContainerKind::Mapping);
                if !has_enclosing_mapping {
                    return Err(self.fail_here(err!(UnexpectedToken), Code::KeyBesideSequence));
                }

                self.close_pending_empty_value()?;
                let end = self.w.span_of(current.id).end;
                let id = self.close_container(end)?;
                self.finish_value(id)?;
                // The sequence's parent is now current; recurse to land in the
                // enclosing mapping.
                return self.ensure_container(kind);
            }
        }

        // An indentless block sequence opened as a mapping value shares that
        // mapping's column.
        let shares_parent = kind == ContainerKind::Sequence
            && !self.force_new_container
            && !self.container_stack.is_empty()
            && self.cur().kind == ContainerKind::Mapping;
        self.force_new_container = false;
        let start = self.peek().span.start;
        let id = self.open_container(kind, start)?;
        if shares_parent {
            self.container_by_id(id).shares_parent_indent = true;
        }
        Ok(id)
    }

    fn open_container(&mut self, kind: ContainerKind, start: usize) -> Result<NodeId> {
        let empty = match kind {
            ContainerKind::Sequence => NodeKind::Sequence(None),
            ContainerKind::Mapping => NodeKind::Mapping(None),
        };
        // A container claims a parked container-slot property; the pending
        // property is preserved for its first child.
        self.parking_container = true;
        let id = if self.container_anchor.is_some() || self.container_tag.is_some() {
            let child_anchor = self.pending_anchor;
            let child_tag = self.pending_tag;
            self.pending_anchor = self.container_anchor.take();
            self.pending_tag = self.container_tag.take();
            let new_id = self.add_node(empty, Span::new(start, start))?;
            self.pending_anchor = child_anchor;
            self.pending_tag = child_tag;
            new_id
        } else {
            self.add_node(empty, Span::new(start, start))?
        };
        self.parking_container = false;

        self.container_stack.push(OpenContainer::new(id, kind));
        Ok(id)
    }

    fn close_container(&mut self, span_end: usize) -> Result<NodeId> {
        let Some(container) = self.container_stack.pop() else { return Err(err!(UnexpectedToken)) };
        if container.first_child.is_none() {
            self.set_end(container.id, span_end);
        }
        Ok(container.id)
    }

    /// True when the current container is mid-entry, waiting for a value.
    fn current_awaits_value(&self) -> bool {
        let Some(c) = self.container_stack.last() else { return false };
        match c.kind {
            ContainerKind::Mapping => c.pending_key.is_some(),
            ContainerKind::Sequence => c.pending_sequence_item,
        }
    }

    /// Attaches a value that may have been written on the line after its key.
    fn attach_deferred_value(&mut self, value_id: NodeId) -> Result<()> {
        if self.force_new_container {
            self.value_only_indents += 1;
            self.force_new_container = false;
        }
        self.finish_value(value_id)
    }

    fn finish_value(&mut self, value_id: NodeId) -> Result<()> {
        // This value is the trailing-comment candidate.
        self.last_value_id = Some(value_id);
        if self.container_stack.is_empty() {
            // A second top-level node means trailing junk after the root.
            if self.root.is_some() {
                return Err(err!(UnexpectedToken));
            }
            self.root = Some(value_id);
            return Ok(());
        }

        let pi = self.container_stack.len() - 1;
        match self.container_stack[pi].kind {
            ContainerKind::Sequence => {
                self.attach_child(pi, value_id)?;
                let parent = &mut self.container_stack[pi];
                parent.pending_sequence_item = false;
                parent.pending_sequence_item_span = None;
                if let Some(m) = parent.current_marker.take() {
                    self.w.marker(value_id, m)?;
                }
            }
            ContainerKind::Mapping => {
                // A complex explicit key just finished: it becomes the pending key.
                if self.container_stack[pi].building_explicit_key {
                    let end = self.w.span_of(value_id).end;
                    let parent = &mut self.container_stack[pi];
                    parent.building_explicit_key = false;
                    parent.pending_key = Some(value_id);
                    parent.pending_value_span = end;
                    parent.explicit_awaiting_value = true;
                    return Ok(());
                }

                let Some(key_id) = self.container_stack[pi].pending_key else {
                    let at = self.w.span_of(value_id).start;
                    return Err(self.fail_at(err!(UnexpectedToken), Code::ContinuationAfterValue, Span::new(at, at)));
                };
                {
                    let parent = &mut self.container_stack[pi];
                    parent.pending_key = None;
                    parent.explicit_awaiting_value = false;
                }

                // A comment parked on the `key:` line binds to this value as its
                // trailing comment.
                if let Some(c) = self.container_stack[pi].pending_value_trailing.take() {
                    self.trailing.push(TrailingComment { id: value_id, comment: c });
                }

                let key_span = self.w.span_of(key_id);
                let value_span = self.w.span_of(value_id);
                // The `keyvalue` pair is a synthetic wrapper, not a node that any
                // comment leads: park like a container node and let the buffer
                // fall through to the sibling's key.
                let prev_parking = self.parking_container;
                self.parking_container = true;
                let pair_id = self.add_node(NodeKind::KeyValue(KeyValue { key: key_id, value: value_id }), Span::new(key_span.start, value_span.end))?;
                self.parking_container = prev_parking;

                if let Some(sep) = self.container_stack[pi].pending_sep_span.take() {
                    self.w.sep(pair_id, sep)?;
                }
                self.attach_child(pi, pair_id)?;
            }
        }
        Ok(())
    }

    fn close_pending_empty_value(&mut self) -> Result<()> {
        let Some(parent) = self.container_stack.last().copied() else { return Ok(()) };
        match parent.kind {
            ContainerKind::Sequence => {
                if parent.pending_sequence_item {
                    let span = parent.pending_sequence_item_span.unwrap_or(0);
                    let value_id = self.add_node(NodeKind::Null, Span::new(span, span))?;
                    self.finish_value(value_id)?;
                }
            }
            ContainerKind::Mapping => {
                if parent.pending_key.is_some() {
                    self.cur_mut().explicit_awaiting_value = false;
                    let value_id = self.add_node(NodeKind::Null, Span::new(parent.pending_value_span, parent.pending_value_span))?;
                    self.finish_value(value_id)?;
                }
            }
        }
        Ok(())
    }

    fn close_sequence_item_continuation(&mut self) -> Result<()> {
        // While descending into a freshly-opened deeper container, the open item
        // continuation is an ANCESTOR being extended, not a sibling to close.
        if self.force_new_container {
            return Ok(());
        }
        // Close every stacked same-line continuation container, not just one.
        while !self.container_stack.is_empty() && self.cur().continues_sequence_item {
            self.cur_mut().continues_sequence_item = false;
            self.close_pending_empty_value()?;
            let end = self.w.span_of(self.cur().id).end;
            let id = self.close_container(end)?;
            self.finish_value(id)?;
        }
        Ok(())
    }

    /// Hands a line beginning at column `col` to the open compact item container
    /// (`continues_sequence_item`) that begins at that column, closing the ones
    /// nested deeper than it on its dash line. True when a compact item begins
    /// at `col`.
    fn land_continuations(&mut self, col: usize) -> Result<bool> {
        while !self.container_stack.is_empty() {
            let c = *self.cur();
            if !c.continues_sequence_item {
                return Ok(false);
            }
            let c_col = self.column_of(self.w.span_of(c.id).start);
            if c_col < col {
                return Ok(false);
            }
            self.cur_mut().continues_sequence_item = false;
            if c_col == col {
                return Ok(true);
            }
            self.close_pending_empty_value()?;
            let end = self.w.span_of(self.cur().id).end;
            let id = self.close_container(end)?;
            self.finish_value(id)?;
        }
        Ok(false)
    }

    /// If a complex explicit key's container is still open — its `:` sits on the
    /// same line, so no dedent closed it — close it now. Only a `:` at (or left
    /// of) the `?`'s own column is that key's value indicator.
    fn close_open_complex_key(&mut self) -> Result<()> {
        let n = self.container_stack.len();
        if n < 2 {
            return Ok(());
        }
        let parent = self.container_stack[n - 2];
        if !parent.building_explicit_key {
            return Ok(());
        }
        if self.column_of(self.peek().span.start) > parent.explicit_key_col {
            return Ok(());
        }

        self.cur_mut().continues_sequence_item = false;
        self.close_pending_empty_value()?;
        let end = self.w.span_of(self.cur().id).end;
        let id = self.close_container(end)?;
        self.finish_value(id)
    }

    fn clear_pending_sequence_item(&mut self, sequence_id: NodeId) {
        let parent = self.container_by_id(sequence_id);
        parent.pending_sequence_item = false;
        parent.pending_sequence_item_span = None;
    }

    fn attach_child(&mut self, pi: usize, child_id: NodeId) -> Result<()> {
        let parent = self.container_stack[pi];
        if parent.first_child.is_some() {
            self.w.set_next(parent.last_child.unwrap(), Some(child_id));
        } else {
            self.container_stack[pi].first_child = Some(child_id);
            match parent.kind {
                ContainerKind::Sequence => self.w.set(parent.id, NodeKind::Sequence(Some(child_id)))?,
                ContainerKind::Mapping => self.w.set(parent.id, NodeKind::Mapping(Some(child_id)))?,
            }
        }

        self.container_stack[pi].last_child = Some(child_id);
        let end = self.w.span_of(child_id).end;
        self.set_end(parent.id, end);
        Ok(())
    }

    /// Move node `id`'s span to end at `end`.
    fn set_end(&mut self, id: NodeId, end: usize) {
        let start = self.w.span_of(id).start;
        self.w.set_span(id, Span::new(start, end));
    }

    fn scalar_kind(&mut self, source: &'s [u8]) -> Result<ScalarKind<'s>> {
        if source.len() >= 2 && source[0] == b'\'' {
            return Ok(ScalarKind::String(self.get_single_quoted_string(source)?));
        }
        if source.len() >= 2 && source[0] == b'"' {
            return Ok(ScalarKind::String(self.get_double_quoted_string(source)?));
        }
        // A multi-line plain scalar folds its line breaks; it is always a string.
        if source.contains(&b'\n') {
            return Ok(ScalarKind::String(Cow::Owned(fold_plain_scalar(source))));
        }
        if self.version == Type::V1_1 {
            return Ok(scalar_kind1_1(source));
        }
        if eql_any(source, &[b"null", b"Null", b"NULL", b"~"]) {
            return Ok(ScalarKind::Other(NodeKind::Null));
        }
        if eql_any(source, &[b"true", b"True", b"TRUE"]) {
            return Ok(ScalarKind::Other(NodeKind::Boolean(true)));
        }
        if eql_any(source, &[b"false", b"False", b"FALSE"]) {
            return Ok(ScalarKind::Other(NodeKind::Boolean(false)));
        }
        match classify_number(source) {
            NumberClass::Integer => return Ok(ScalarKind::Other(NodeKind::Number(Number { raw: source, kind: NumberKind::Integer }))),
            NumberClass::Float => return Ok(ScalarKind::Other(NodeKind::Number(Number { raw: source, kind: NumberKind::Float }))),
            NumberClass::NotNumber => {}
        }
        Ok(ScalarKind::String(Cow::Borrowed(source)))
    }

    fn get_single_quoted_string(&mut self, source: &'s [u8]) -> Result<Cow<'s, [u8]>> {
        if source.len() < 2 || source[0] != b'\'' || source[source.len() - 1] != b'\'' {
            return Err(err!(UnclosedString));
        }
        let inner = &source[1..source.len() - 1];

        if !inner.contains(&b'\'') && !inner.contains(&b'\n') {
            return Ok(Cow::Borrowed(inner));
        }

        let mut decoded: Vec<u8> = Vec::new();
        let mut index = 0usize;
        while index < inner.len() {
            // `\r\n` is a single line break: drop the `\r`.
            if inner[index] == b'\r' && index + 1 < inner.len() && inner[index + 1] == b'\n' {
                index += 1;
                continue;
            }
            if inner[index] == b'\n' {
                index = fold_flow_break(&mut decoded, inner, index);
                continue;
            }
            if inner[index] != b'\'' {
                decoded.push(inner[index]);
                index += 1;
                continue;
            }

            if index + 1 >= inner.len() || inner[index + 1] != b'\'' {
                return Err(err!(UnexpectedToken));
            }
            decoded.push(b'\'');
            index += 2;
        }

        Ok(Cow::Owned(decoded))
    }

    fn get_double_quoted_string(&mut self, source: &'s [u8]) -> Result<Cow<'s, [u8]>> {
        if source.len() < 2 || source[0] != b'"' || source[source.len() - 1] != b'"' {
            return Err(err!(UnclosedString));
        }
        let inner = &source[1..source.len() - 1];

        if !inner.contains(&b'\\') && !inner.contains(&b'\n') {
            return Ok(Cow::Borrowed(inner));
        }

        let mut decoded: Vec<u8> = Vec::new();
        let mut index = 0usize;
        while index < inner.len() {
            let ch = inner[index];
            // `\r\n` is a single line break: drop the `\r`.
            if ch == b'\r' && index + 1 < inner.len() && inner[index + 1] == b'\n' {
                index += 1;
                continue;
            }
            if ch == b'\n' {
                index = fold_flow_break(&mut decoded, inner, index);
                continue;
            }
            if ch != b'\\' {
                decoded.push(ch);
                index += 1;
                continue;
            }

            index += 1;
            if index >= inner.len() {
                return Err(err!(UnclosedString));
            }

            // An escaped line break joins the lines directly, dropping the
            // leading whitespace of the continuation.
            if inner[index] == b'\r' && index + 1 < inner.len() && inner[index + 1] == b'\n' {
                index += 1;
            }
            if inner[index] == b'\n' {
                index += 1;
                while index < inner.len() && (inner[index] == b' ' || inner[index] == b'\t') {
                    index += 1;
                }
                continue;
            }

            match inner[index] {
                b'0' => decoded.push(0x00),
                b'a' => decoded.push(0x07),
                b'b' => decoded.push(0x08),
                b't' | b'\t' => decoded.push(b'\t'),
                b'n' => decoded.push(b'\n'),
                b'v' => decoded.push(0x0b),
                b'f' => decoded.push(0x0c),
                b'r' => decoded.push(b'\r'),
                b'e' => decoded.push(0x1b),
                b' ' => decoded.push(b' '),
                b'"' => decoded.push(b'"'),
                b'/' => decoded.push(b'/'),
                b'\\' => decoded.push(b'\\'),
                b'N' => append_codepoint(&mut decoded, 0x85)?,
                b'_' => append_codepoint(&mut decoded, 0xA0)?,
                b'L' => append_codepoint(&mut decoded, 0x2028)?,
                b'P' => append_codepoint(&mut decoded, 0x2029)?,
                b'x' | b'u' | b'U' => {
                    let digits: usize = match inner[index] {
                        b'x' => 2,
                        b'u' => 4,
                        _ => 8,
                    };
                    let codepoint = match parse_hex_escape(inner, index + 1, digits) {
                        Ok(c) => c,
                        Err(e) => return Err(self.escape_fail(e, inner, index, digits)),
                    };
                    if let Err(e) = append_codepoint(&mut decoded, codepoint) {
                        return Err(self.escape_fail(e, inner, index, digits));
                    }
                    index += digits;
                }
                _ => {
                    // Point at the backslash and the character it failed to escape.
                    if let Some(base) = self.source_offset_of(inner) {
                        return Err(self.fail_at(err!(UnexpectedToken), Code::InvalidEscape, Span::new(base + index - 1, base + index + 1)));
                    }
                    return Err(err!(UnexpectedToken));
                }
            }
            index += 1;
        }

        Ok(Cow::Owned(decoded))
    }

    /// Pin a failed `\x`/`\u`/`\U` escape on its own bytes — the backslash, the
    /// letter, and up to `digits` hex digits — and hand `e` back to return.
    fn escape_fail(&mut self, e: Error, inner: &[u8], index: usize, digits: usize) -> Error {
        if e.is("OutOfMemory") {
            return e;
        }
        if let Some(base) = self.source_offset_of(inner) {
            let end = (index + 1 + digits).min(inner.len());
            self.failure = Some(Diagnostic { code: code_for(e), offset: base + index - 1, end: Some(base + end) });
        }
        e
    }

    fn is_mapping_start(&self) -> bool {
        match self.peek().kind {
            // An implicit block mapping key must be a single line.
            TK::Scalar => {
                if self.peek().source(self.source).contains(&b'\n') {
                    return false;
                }
            }
            // An alias may be a key (`*a : b`).
            TK::Alias => {}
            _ => return false,
        }

        let mut lookahead = self.index + 1;
        while lookahead < self.tokens.len() {
            match self.tokens[lookahead].kind {
                TK::Whitespace | TK::Comment => {}
                TK::Colon => return true,
                _ => return false,
            }
            lookahead += 1;
        }
        false
    }

    fn add_node(&mut self, kind: NodeKind<'_>, span: Span) -> Result<NodeId> {
        // Consume any pending node properties onto this node.
        let tag = self.pending_tag.take();
        let anchor = self.pending_anchor.take();
        let mut node_span = span;
        // Extend the node's span leftward to cover its property prefix.
        if let Some(t) = tag {
            node_span.start = node_span.start.min(t.span.start);
        }
        if let Some(a) = anchor {
            node_span.start = node_span.start.min(a.span.start);
        }
        // An implicit empty (`k: !!str`, `- &a`) is anchored at the separator,
        // before its properties: it covers them instead, as a written value does.
        if span.start == span.end {
            let mut after: Option<Span> = None;
            if let Some(t) = tag {
                if t.span.start >= span.start {
                    after = Some(t.span);
                }
            }
            if let Some(a) = anchor {
                if a.span.start >= span.start {
                    after = Some(match after {
                        Some(b) => Span::new(b.start.min(a.span.start), b.end.max(a.span.end)),
                        None => a.span,
                    });
                }
            }
            if let Some(b) = after {
                node_span = b;
            }
        }
        // A `!!binary` scalar is a byte string: its text, less the line breaks
        // and indentation a block scalar wraps it in, is the base64 every
        // format's `data` holds. Text that is not base64 stays a string.
        let mut binary: Vec<u8> = Vec::new();
        let mut is_data = false;
        if let Some(t) = tag {
            if let NodeKind::String(text) = kind {
                if let Some(name) = core_tag_name(&self.tag_directives, t.text) {
                    if name == b"binary" {
                        binary.extend(text.iter().copied().filter(|c| !is_ascii_whitespace(*c)));
                        if is_base64(&binary) {
                            is_data = true;
                        }
                    }
                }
            }
        }
        let node_kind = if is_data { NodeKind::Extended(Extended { kind: ExtKind::Data, text: &binary }) } else { kind };
        let id = self.w.add(node_kind, node_span)?;
        // YAML stores the tag VERBATIM, resolved here under the document's `%TAG`
        // directives, which all precede the body.
        if let Some(t) = tag {
            let res = resolve_tag(&self.tag_directives, t.text, node_kind);
            self.w.tag(id, Tag::Text(t.text), res.resolved, res.agrees, Some(t.span))?;
        }
        if let Some(a) = anchor {
            self.w.anchor(id, a.name, Some(a.span))?;
            self.anchors.push(AnchorDef { name: a.name, node: id });
        }
        // Buffered leading comments bind to the first non-container node built
        // after them.
        if !self.parking_container {
            self.claim_leading(id)?;
        }
        Ok(id)
    }

    /// Drain leading node-property tokens onto the parser's pending state, to be
    /// attached to the next node by `add_node`.
    fn consume_pending_properties(&mut self) -> Result<()> {
        loop {
            self.skip_trivia_no_newline();
            match self.peek().kind {
                TK::Tag => {
                    let t = self.advance();
                    self.stash_tag(t.source(self.source), t.span)?;
                }
                TK::Anchor => {
                    let a = self.advance();
                    // Strip the leading `&`; the name is the rest of the token.
                    self.stash_anchor(&a.source(self.source)[1..], a.span)?;
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// True when a pending anchor/tag property sits on the same source line as
    /// the token starting at `at` (no newline between them).
    fn pending_prop_on_line_of(&self, at: usize) -> bool {
        let same_line = |src: &[u8], from: usize, to: usize| from <= to && !src[from..to].contains(&b'\n');
        if let Some(p) = self.pending_anchor {
            if same_line(self.source, p.span.end, at) {
                return true;
            }
        }
        if let Some(p) = self.pending_tag {
            if same_line(self.source, p.span.end, at) {
                return true;
            }
        }
        false
    }

    /// Record an anchor as pending. A second pending anchor means the first
    /// belongs to a container about to open (parked in `container_anchor`); a
    /// third is a real duplicate on one node.
    fn stash_anchor(&mut self, name: &'s [u8], span: Span) -> Result<()> {
        if let Some(prev) = self.pending_anchor {
            if self.container_anchor.is_some() {
                return Err(self.fail_at(err!(DuplicateProperty), Code::DuplicateProperty, span));
            }
            self.container_anchor = Some(prev);
            self.pending_anchor = None;
        }
        self.pending_anchor = Some(PendingAnchor { name, span });
        Ok(())
    }

    /// A tag using a *named* handle (`!handle!suffix`) is valid only if that
    /// handle was declared by a `%TAG` directive in this document.
    fn validate_tag_handle(&self, text: &[u8]) -> Result<()> {
        if text.len() < 2 {
            return Ok(()); // `!` non-specific tag
        }
        if text[1] == b'<' || text[1] == b'!' {
            return Ok(()); // verbatim, or secondary `!!`
        }
        // A named handle has a second `!` closing it (`!e!foo` → handle `!e!`).
        let Some(close) = text[1..].iter().position(|&c| c == b'!').map(|p| p + 1) else { return Ok(()) };
        let handle = &text[..close + 1];
        if self.tag_directives.iter().any(|d| d.handle == handle) {
            return Ok(());
        }
        Err(err!(UndefinedTagHandle))
    }

    fn stash_tag(&mut self, text: &'s [u8], span: Span) -> Result<()> {
        if let Err(e) = self.validate_tag_handle(text) {
            return Err(self.fail_at(e, Code::UndefinedTagHandle, span));
        }
        if let Some(prev) = self.pending_tag {
            if self.container_tag.is_some() {
                return Err(self.fail_at(err!(DuplicateProperty), Code::DuplicateProperty, span));
            }
            self.container_tag = Some(prev);
            self.pending_tag = None;
        }
        self.pending_tag = Some(PendingTag { text, span });
        Ok(())
    }

    fn cur(&self) -> &OpenContainer<'s> {
        self.container_stack.last().unwrap()
    }

    fn cur_mut(&mut self) -> &mut OpenContainer<'s> {
        self.container_stack.last_mut().unwrap()
    }

    fn container_by_id(&mut self, id: NodeId) -> &mut OpenContainer<'s> {
        self.container_stack.iter_mut().find(|c| c.id == id).expect("an open container")
    }

    fn skip_trivia_no_newline(&mut self) {
        while let TK::Whitespace | TK::Comment = self.peek().kind {
            self.advance();
        }
    }

    fn peek(&self) -> Token {
        self.tokens[self.index]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.index];
        self.index += 1;
        match token.kind {
            // The single choke point for comment trivia.
            TK::Comment => self.capture_comment(token),
            // A newline closes the current value's trailing-comment window.
            TK::Newline => {
                self.last_value_id = None;
                self.colon_line = false;
            }
            // A value-indicator opens the window in which a same-line comment
            // trails the entry rather than leading its (block) value's first child.
            TK::Colon => self.colon_line = true,
            _ => {}
        }
        token
    }

    // ── comments ────────────────────────────────────────────────────────────

    /// Classify the just-advanced comment token. While the previous value's
    /// window is open the comment is on the same line and trails it; otherwise
    /// it buffers as leading for the next node.
    fn capture_comment(&mut self, token: Token) {
        let c = Comment { text: comment_text(token.source(self.source)), style: Style::Line };
        if let Some(id) = self.last_value_id {
            self.trailing.push(TrailingComment { id, comment: c });
            self.last_value_id = None; // one trailing per value
        } else if self.colon_line && self.awaits_block_value() {
            // `key: # note` with the value on following lines — the comment
            // trails the entry. Bound to the value in `finish_value`.
            self.cur_mut().pending_value_trailing = Some(c);
        } else {
            self.pending_leading.push(c);
        }
    }

    /// True when the current container is a mapping with a recorded key but no
    /// value yet.
    fn awaits_block_value(&self) -> bool {
        let Some(c) = self.container_stack.last() else { return false };
        c.kind == ContainerKind::Mapping && c.pending_key.is_some()
    }

    /// Hand the buffered leading comments to node `id`, then clear the buffer.
    fn claim_leading(&mut self, id: NodeId) -> Result<()> {
        for c in self.pending_leading.drain(..) {
            self.w.comment(id, table::COMMENT_LEADING, c)?;
        }
        Ok(())
    }

    /// Hand buffered orphan comments (no node followed them — at end of
    /// document) to `id` as its `dangling` run.
    fn claim_dangling(&mut self, id: NodeId) -> Result<()> {
        for c in self.pending_leading.drain(..) {
            self.w.comment(id, table::COMMENT_DANGLING, c)?;
        }
        Ok(())
    }
}

/// The offset of a tab in the leading whitespace of the line holding `pos`,
/// or `None` when that line is indented with spaces alone.
fn leading_tab(source: &[u8], pos: usize) -> Option<usize> {
    let at = pos.min(source.len());
    let line_start = source[..at].iter().rposition(|&c| c == b'\n').map_or(0, |nl| nl + 1);
    let mut i = line_start;
    while i < source.len() && (source[i] == b' ' || source[i] == b'\t') {
        if source[i] == b'\t' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn is_directive_space(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\r'
}

fn skip_directive_space(line: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < line.len() && is_directive_space(line[j]) {
        j += 1;
    }
    j
}

/// A `%YAML` version is `major.minor`, both non-empty digit runs.
fn is_yaml_version(s: &[u8]) -> bool {
    let Some(dot) = s.iter().position(|&c| c == b'.') else { return false };
    let major = &s[..dot];
    let minor = &s[dot + 1..];
    if major.is_empty() || minor.is_empty() {
        return false;
    }
    major.iter().all(|c| c.is_ascii_digit()) && minor.iter().all(|c| c.is_ascii_digit())
}

/// After a directive's parameters only blanks and an optional `#` comment may
/// follow.
fn require_directive_end(line: &[u8], i: usize) -> Result<()> {
    let j = skip_directive_space(line, i);
    if j < line.len() && line[j] != b'#' {
        return Err(err!(InvalidDirective));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockStyle {
    Literal,
    Folded,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockChomp {
    Clip,
    Strip,
    Keep,
}

fn decode_block_scalar(header: &[u8], parent_indent: usize, body: &[u8]) -> Vec<u8> {
    let style = if header[0] == b'|' { BlockStyle::Literal } else { BlockStyle::Folded };
    let mut chomp = BlockChomp::Clip;
    let mut explicit: Option<usize> = None;
    for &c in &header[1..] {
        match c {
            b'-' => chomp = BlockChomp::Strip,
            b'+' => chomp = BlockChomp::Keep,
            b'1'..=b'9' => explicit = Some((c - b'0') as usize),
            _ => {}
        }
    }

    let content_indent: usize = match explicit {
        Some(d) => parent_indent + d,
        None => autodetect_indent(body),
    };

    // Split into physical lines, dropping the empty segment a trailing newline
    // would otherwise create.
    let ends_nl = !body.is_empty() && body[body.len() - 1] == b'\n';
    let work = if ends_nl { &body[..body.len() - 1] } else { body };

    let mut total_lines: usize = 0;
    let mut last_content: Option<usize> = None;
    for (idx, line) in LineIter::new(work).enumerate() {
        if !is_all_whitespace(line) {
            last_content = Some(idx);
        }
        total_lines += 1;
    }

    let mut decoded: Vec<u8> = Vec::new();

    if let Some(last) = last_content {
        let mut emitted = false;
        let mut prev_blank = false;
        let mut prev_more = false;
        for (idx, line) in LineIter::new(work).enumerate() {
            if idx > last {
                break; // trailing blanks are decided by chomping
            }
            let blank = is_all_whitespace(line);
            let text: &[u8] = if blank { b"" } else { dedent_line(line, content_indent) };
            let more = !text.is_empty() && text[0] == b' ';

            if style == BlockStyle::Literal {
                if emitted {
                    decoded.push(b'\n');
                }
                decoded.extend_from_slice(text);
            } else if !emitted {
                decoded.extend_from_slice(text);
            } else if blank {
                decoded.push(b'\n');
            } else {
                if prev_blank {
                    // The blank line already emitted the break.
                } else if prev_more || more {
                    decoded.push(b'\n');
                } else {
                    decoded.push(b' ');
                }
                decoded.extend_from_slice(text);
            }

            emitted = true;
            prev_blank = blank;
            prev_more = more && !blank;
        }

        let trailing_blanks = total_lines - 1 - last;
        let present_breaks = trailing_blanks + if ends_nl { 1 } else { 0 };
        let keep_breaks: usize = match chomp {
            BlockChomp::Strip => 0,
            BlockChomp::Clip => present_breaks.min(1),
            BlockChomp::Keep => present_breaks,
        };
        decoded.extend(std::iter::repeat(b'\n').take(keep_breaks));
    } else if chomp == BlockChomp::Keep {
        let present_breaks = total_lines.saturating_sub(1) + if ends_nl { 1 } else { 0 };
        decoded.extend(std::iter::repeat(b'\n').take(present_breaks));
    }

    decoded
}

struct LineIter<'a> {
    source: &'a [u8],
    i: usize,
    done: bool,
}

impl<'a> LineIter<'a> {
    fn new(source: &'a [u8]) -> Self {
        LineIter { source, i: 0, done: false }
    }
}

impl<'a> Iterator for LineIter<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.done {
            return None;
        }
        let start = self.i;
        while self.i < self.source.len() && self.source[self.i] != b'\n' {
            self.i += 1;
        }
        // `\r\n` is a single line break: exclude a trailing `\r`.
        let mut end = self.i;
        if end > start && self.source[end - 1] == b'\r' {
            end -= 1;
        }
        let line = &self.source[start..end];
        if self.i < self.source.len() {
            self.i += 1;
        } else {
            self.done = true;
        }
        Some(line)
    }
}

fn is_all_whitespace(line: &[u8]) -> bool {
    line.iter().all(|&c| c == b' ' || c == b'\t')
}

fn dedent_line(line: &[u8], n: usize) -> &[u8] {
    let mut k = 0usize;
    while k < n && k < line.len() && line[k] == b' ' {
        k += 1;
    }
    &line[k..]
}

fn autodetect_indent(body: &[u8]) -> usize {
    for line in LineIter::new(body) {
        if is_all_whitespace(line) {
            continue;
        }
        let mut k = 0usize;
        while k < line.len() && line[k] == b' ' {
            k += 1;
        }
        return k;
    }
    0
}

/// A plain scalar inside flow may not begin with a comment indicator, and the
/// indicators `-`/`?`/`:` are valid first characters only when followed by
/// plain-safe content (`ns-plain-first`). Without this check `{b: - a}` reads
/// as the STRING `"- a"`.
fn invalid_flow_scalar(source: &[u8]) -> bool {
    if source.is_empty() {
        return true;
    }
    if source[0] == b'#' {
        return true;
    }
    let indicator = source[0] == b'-' || source[0] == b'?' || source[0] == b':';
    if !indicator {
        return false;
    }
    if source.len() == 1 {
        return true;
    }
    source[1] == b' ' || source[1] == b'\t'
}

// ── YAML 1.1 scalar type resolution ─────────────────────────────────────────
//
// 1.1 resolves plain scalars by the tag repository (yaml.org/type), which
// diverges from 1.2's core schema: `yes`/`no`/`on`/`off`/`y`/`n` booleans,
// leading-zero octal (`0777`) + binary (`0b…`) + sexagesimal (`190:20:30`)
// ints, `_` digit separators, floats requiring a `.` with a *signed* exponent,
// and `!!timestamp` auto-resolution. The lexeme stays in `number.raw` /
// `extended.text`; value normalization is the consumer's job.
fn scalar_kind1_1(source: &[u8]) -> ScalarKind<'_> {
    if eql_any(source, &[b"null", b"Null", b"NULL", b"~"]) {
        return ScalarKind::Other(NodeKind::Null);
    }
    if let Some(b) = bool1_1(source) {
        return ScalarKind::Other(NodeKind::Boolean(b));
    }
    match classify1_1_number(source) {
        NumberClass::Integer => return ScalarKind::Other(NodeKind::Number(Number { raw: source, kind: NumberKind::Integer })),
        NumberClass::Float => return ScalarKind::Other(NodeKind::Number(Number { raw: source, kind: NumberKind::Float })),
        NumberClass::NotNumber => {}
    }
    if let Some(kind) = classify1_1_timestamp(source) {
        return ScalarKind::Other(NodeKind::Extended(Extended { kind, text: source }));
    }
    ScalarKind::String(Cow::Borrowed(source))
}

/// YAML 1.1 boolean tag: `y|Y|yes|Yes|YES|true|True|TRUE|on|On|ON` (true) and
/// the matching negatives (false). Only these exact spellings.
fn bool1_1(source: &[u8]) -> Option<bool> {
    if eql_any(source, &[b"y", b"Y", b"yes", b"Yes", b"YES", b"true", b"True", b"TRUE", b"on", b"On", b"ON"]) {
        return Some(true);
    }
    if eql_any(source, &[b"n", b"N", b"no", b"No", b"NO", b"false", b"False", b"FALSE", b"off", b"Off", b"OFF"]) {
        return Some(false);
    }
    None
}

/// Whether `s` is standard base64: the alphabet, then at most two `=`, in
/// whole groups of four. Empty is the empty byte string.
fn is_base64(s: &[u8]) -> bool {
    if s.len() % 4 != 0 {
        return false;
    }
    let mut i = 0usize;
    while i < s.len() && s[i] != b'=' {
        let c = s[i];
        if !c.is_ascii_alphanumeric() && c != b'+' && c != b'/' {
            return false;
        }
        i += 1;
    }
    let pad = s.len() - i;
    if pad > 2 {
        return false;
    }
    s[i..].iter().all(|&c| c == b'=')
}

/// Zig's `std.ascii.isWhitespace`: space, `\t`, `\n`, `\r`, `\x0b`, `\x0c`.
fn is_ascii_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

// ── Tag resolution ──────────────────────────────────────────────────────────
// YAML resolves its own tags: each row's tag carries its text as written, what
// it names under the document's `%TAG` directives — a core type, custom, or
// YAML's non-specific `!` — and whether the node's text agrees with the type.

/// What a language made of a text tag: `table::TAG_*`, and whether the
/// node's text agrees with the kind it names (`!!int` on `1 - 3` does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagResolution {
    pub resolved: u8,
    pub agrees: bool,
}

impl Default for TagResolution {
    fn default() -> Self {
        TagResolution { resolved: table::TAG_UNSTATED, agrees: true }
    }
}

/// What the tag `text` on a node of `kind` resolves to under `directives`.
pub fn resolve_tag(directives: &[TagDirective<'_>], text: &[u8], kind: NodeKind<'_>) -> TagResolution {
    if text == b"!" {
        return TagResolution { resolved: table::TAG_NON_SPECIFIC, agrees: true };
    }
    let Some(name) = core_tag_name(directives, text) else {
        return TagResolution { resolved: table::TAG_CUSTOM, agrees: true };
    };
    // `!!binary` on a byte string has been read into the node's own kind
    // (`data`), and asks nothing more of it.
    if name == b"binary" {
        if let NodeKind::Extended(e) = kind {
            if e.kind == ExtKind::Data {
                return TagResolution { resolved: table::TAG_NON_SPECIFIC, agrees: true };
            }
        }
    }
    let k = match name {
        b"str" => KindTag::String,
        b"null" => KindTag::Null,
        b"bool" => KindTag::Boolean,
        b"int" => KindTag::Integer,
        b"float" => KindTag::Float,
        b"seq" => KindTag::Sequence,
        b"map" => KindTag::Mapping,
        // A `!!`-secondary tag with a name the core schema does not have
        // (`!!binary`, `!!timestamp`) is custom.
        _ => return TagResolution { resolved: table::TAG_CUSTOM, agrees: true },
    };
    TagResolution { resolved: table::TAG_KIND + k.ordinal(), agrees: agrees(k, kind) }
}

/// Whether a node of `kind` can be what the core type `k` says.
fn agrees(k: KindTag, kind: NodeKind<'_>) -> bool {
    let collection = matches!(kind, NodeKind::Sequence(_) | NodeKind::Mapping(_));
    match k {
        KindTag::Sequence => return matches!(kind, NodeKind::Sequence(_)),
        KindTag::Mapping => return matches!(kind, NodeKind::Mapping(_)),
        _ => {
            if collection {
                return false;
            }
        }
    }
    let t: &[u8] = match kind {
        NodeKind::String(x) => x,
        NodeKind::Number(n) => n.raw,
        NodeKind::Boolean(b) => {
            if b {
                b"true"
            } else {
                b"false"
            }
        }
        NodeKind::Null => b"null",
        _ => b"",
    };
    match k {
        KindTag::String | KindTag::Null => true,
        KindTag::Boolean => eql_any(t, &[b"true", b"True", b"TRUE", b"false", b"False", b"FALSE"]),
        KindTag::Integer => classify_number(t) == NumberClass::Integer || classify1_1_number(t) == NumberClass::Integer,
        KindTag::Float => (classify_number(t) != NumberClass::NotNumber && !has_radix_prefix(t)) || classify1_1_number(t) == NumberClass::Float,
        KindTag::Sequence | KindTag::Mapping => unreachable!(),
    }
}

/// YAML 1.1 `!!timestamp`: a `YYYY-MM-DD` date or a full date-time with an
/// optional `Z`/`±HH:MM` zone. Time-only and minute-precision forms are
/// refused. Returns the matching extended kind, or `None`.
fn classify1_1_timestamp(source: &[u8]) -> Option<ExtKind> {
    let kind = datetime::classify(source, datetime::Options { allow_time_only: false, allow_minute_precision: false }).ok()?;
    Some(match kind {
        datetime::Kind::OffsetDatetime => ExtKind::OffsetDatetime,
        datetime::Kind::LocalDatetime => ExtKind::LocalDatetime,
        datetime::Kind::LocalDate => ExtKind::LocalDate,
        datetime::Kind::LocalTime => ExtKind::LocalTime,
    })
}

/// Folds a multi-line plain scalar: line breaks fold like flow scalars (a lone
/// break becomes a space, a blank line a newline) with continuation-line
/// indentation stripped.
fn fold_plain_scalar(source: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < source.len() {
        // `\r\n` is a single line break: drop the `\r`.
        if source[i] == b'\r' && i + 1 < source.len() && source[i + 1] == b'\n' {
            i += 1;
            continue;
        }
        if source[i] == b'\n' {
            i = fold_flow_break(&mut out, source, i);
            continue;
        }
        out.push(source[i]);
        i += 1;
    }
    out
}

/// Folds a run of line breaks in a flow (quoted) scalar where `inner[at]` is a
/// newline: trailing whitespace of the line just ended is dropped, the leading
/// whitespace of each continuation line is skipped, a lone break becomes a
/// space, and each additional (blank) line becomes a newline. Returns the index
/// past the run.
fn fold_flow_break(out: &mut Vec<u8>, inner: &[u8], at: usize) -> usize {
    while let Some(&last) = out.last() {
        if last != b' ' && last != b'\t' {
            break;
        }
        out.pop();
    }

    let mut i = at + 1;
    let mut breaks: usize = 1;
    loop {
        while i < inner.len() && (inner[i] == b' ' || inner[i] == b'\t') {
            i += 1;
        }
        // A blank continuation line ending in `\r\n` is still just one break.
        if i < inner.len() && inner[i] == b'\r' && i + 1 < inner.len() && inner[i + 1] == b'\n' {
            i += 1;
        }
        if i < inner.len() && inner[i] == b'\n' {
            breaks += 1;
            i += 1;
            continue;
        }
        break;
    }

    if breaks == 1 {
        out.push(b' ');
    } else {
        out.extend(std::iter::repeat(b'\n').take(breaks - 1));
    }
    i
}

/// Zig's `std.fmt.parseInt(u21, s, 16)`, as an escape reads its hex digits:
/// an optional sign, `_` between digits, and a value that fits 21 bits.
fn parse_hex_escape(source: &[u8], start: usize, digits: usize) -> Result<u32> {
    if start + digits > source.len() {
        return Err(err!(UnclosedString));
    }
    zig_parse_u21_hex(&source[start..start + digits]).ok_or(err!(InvalidUnicodeEscape))
}

fn zig_parse_u21_hex(buf: &[u8]) -> Option<u32> {
    const MAX: u32 = 0x1F_FFFF;
    if buf.is_empty() {
        return None;
    }
    let (neg, body) = match buf[0] {
        b'+' => (false, &buf[1..]),
        b'-' => (true, &buf[1..]),
        _ => (false, buf),
    };
    if body.is_empty() || body[0] == b'_' || body[body.len() - 1] == b'_' {
        return None;
    }
    let mut acc: u32 = 0;
    for &c in body {
        if c == b'_' {
            continue;
        }
        let digit = (c as char).to_digit(16)?;
        if acc != 0 {
            acc *= 16;
            if acc > MAX {
                return None;
            }
        } else if neg {
            // The first digit of a negative number: only a zero fits an
            // unsigned result, and the accumulator stays zero.
            if digit != 0 {
                return None;
            }
            continue;
        }
        acc += digit;
        if acc > MAX {
            return None;
        }
    }
    Some(acc)
}

fn append_codepoint(decoded: &mut Vec<u8>, codepoint: u32) -> Result<()> {
    unicode::encode_append(decoded, codepoint).map_err(|e| if e.is("InvalidCodepoint") { err!(InvalidUnicodeEscape) } else { e })
}

/// True when a plain scalar token cannot legally begin a YAML plain scalar:
/// a leading `,` or the explicit-key indicator `? `.
fn invalid_plain_start(source: &[u8]) -> bool {
    if source.is_empty() {
        return false;
    }
    match source[0] {
        b',' => true,
        b'?' => source.len() >= 2 && (source[1] == b' ' || source[1] == b'\t'),
        _ => false,
    }
}

/// Strip the leading `#` and surrounding spaces from a comment token's bytes.
fn comment_text(raw: &[u8]) -> &[u8] {
    let body = if !raw.is_empty() && raw[0] == b'#' { &raw[1..] } else { raw };
    trim(body, b" \t\r")
}

fn trim<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && set.contains(&s[a]) {
        a += 1;
    }
    while b > a && set.contains(&s[b - 1]) {
        b -= 1;
    }
    &s[a..b]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::ast::{Literal, Node, NodeTag, PathSegment, AST};

    fn p(src: &str) -> Document {
        parse(src.as_bytes(), Type::V1_2_2).unwrap_or_else(|e| panic!("{src:?}: {e:?}"))
    }

    fn p11(src: &str) -> Document {
        parse(src.as_bytes(), Type::V1_1).unwrap()
    }

    fn k(s: &str) -> PathSegment<'_> {
        PathSegment::Key(s.as_bytes())
    }

    fn ix(n: usize) -> PathSegment<'static> {
        PathSegment::Index(n)
    }

    fn val<'a>(doc: &'a Document, path: &[PathSegment<'_>]) -> Node<'a> {
        doc.ast.get_val_by_path(path).unwrap()
    }

    fn s<'a>(doc: &'a Document, path: &[PathSegment<'_>]) -> &'a [u8] {
        val(doc, path).kind.as_string().unwrap_or_else(|| panic!("not a string: {:?}", val(doc, path).kind))
    }

    fn tag(doc: &Document, path: &[PathSegment<'_>]) -> NodeTag {
        val(doc, path).kind.tag()
    }

    fn raw<'a>(doc: &'a Document, path: &[PathSegment<'_>]) -> &'a [u8] {
        val(doc, path).kind.as_number().unwrap().raw
    }

    fn root(doc: &Document) -> Node<'_> {
        doc.ast.node(doc.ast.root)
    }

    fn first_pair(ast: &AST, id: NodeId) -> KeyValue {
        ast.node(ast.node(id).kind.as_mapping().unwrap().unwrap()).kind.as_keyvalue().unwrap()
    }

    /// The text node `id`'s tag was written as, whatever it resolved to.
    fn tag_text(ast: &AST, id: NodeId) -> Option<&[u8]> {
        let f = ast.tag_fact(id)?;
        ast.text_of(f.text)
    }

    fn test_parser_error(input: &str, expected: &str) {
        match parse(input.as_bytes(), Type::V1_2_2) {
            Ok(_) => panic!("{input:?} parsed; expected {expected}"),
            Err(e) => assert_eq!(expected, e.name(), "{input:?}"),
        }
    }

    /// `input` fails with `expected` — the same error `parse` returns — and
    /// `parse_with_report` locates it at 1-based `line`:`column` with `code`.
    fn expect_report(input: &str, expected: &str, code: Code, line: usize, column: usize) {
        test_parser_error(input, expected);
        let mut report = Report::default();
        let e = parse_with_report(input.as_bytes(), Type::V1_2_2, &mut report).unwrap_err();
        assert_eq!(expected, e.name());
        let d = report.diag.expect("a diagnostic");
        let loc = d.locate(input.as_bytes());
        assert_eq!((code, line, column), (d.code, loc.line, loc.column), "{input:?}");
    }

    /// A parse's structure-only shape (`[`/`]` sequence, `{`/`}` mapping, `S`
    /// scalar/alias leaf).
    fn shape(src: &str) -> String {
        let doc = p(src);
        let mut out = String::new();
        shape_node(&doc.ast, doc.ast.node(doc.ast.root), &mut out);
        out
    }

    fn shape_node(ast: &AST, node: Node<'_>, out: &mut String) {
        match node.kind {
            NodeKind::Sequence(first) => {
                shape_push(out, "[");
                let mut child = first;
                while let Some(cid) = child {
                    shape_node(ast, ast.node(cid), out);
                    child = ast.node(cid).next_sibling;
                }
                shape_push(out, "]");
            }
            NodeKind::Mapping(first) => {
                shape_push(out, "{");
                let mut child = first;
                while let Some(cid) = child {
                    let kv = ast.node(cid).kind.as_keyvalue().unwrap();
                    shape_node(ast, ast.node(kv.key), out);
                    shape_node(ast, ast.node(kv.value), out);
                    child = ast.node(cid).next_sibling;
                }
                shape_push(out, "}");
            }
            NodeKind::KeyValue(_) => unreachable!(),
            _ => shape_push(out, "S"),
        }
    }

    fn shape_push(out: &mut String, tok: &str) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tok);
    }

    fn expect_shape(src: &str, want: &str) {
        assert_eq!(want, shape(src), "{src:?}");
    }

    #[test]
    fn parse_with_report_locates_each_class_of_yaml_error() {
        use Code::*;
        // Flow collections: the unclosed one is reported at its opener.
        expect_report("a: [\n", "UnexpectedToken", UnclosedFlowSequence, 1, 4);
        expect_report("x: [a, b\n", "UnexpectedToken", UnclosedFlowSequence, 1, 4);
        expect_report("a: {b: 1\n", "UnexpectedToken", UnclosedFlowMapping, 1, 4);
        expect_report("outer:\n  inner: [1, {a: 2}\n", "UnexpectedToken", UnclosedFlowSequence, 2, 10);
        expect_report("a: [1, 2\nb: 3\n", "UnexpectedToken", UnclosedFlowSequence, 1, 4);
        expect_report("outer:\n  inner: [1, {a: 2}\nnext: 3\n", "InvalidIndent", FlowIndent, 3, 1);
        expect_report("x: [a]]\n", "UnexpectedToken", UnmatchedFlowClose, 1, 7);
        expect_report("]\n", "UnexpectedToken", UnmatchedFlowClose, 1, 1);
        expect_report("x: [\"b\" c]\n", "UnexpectedToken", FlowSequenceSeparator, 1, 9);
        expect_report("x: {a: \"1\" b: 2}\n", "UnexpectedToken", FlowMappingSeparator, 1, 12);
        expect_report("a: [- x]\n", "UnexpectedToken", IndicatorInFlow, 1, 5);
        expect_report("flow: [a,\nb]\n", "InvalidIndent", FlowIndent, 2, 1);

        // Indentation and tabs.
        expect_report("a:\n  b: 1\n c: 2\n", "InvalidIndent", InvalidIndent, 3, 2);
        expect_report("a:\n\tb: 1\n", "TabIndent", TabIndent, 2, 1);
        expect_report("- a\n\t- b\n", "TabIndent", TabIndent, 2, 1);
        expect_report("foo: |\n\t bar\n", "UnexpectedToken", TabIndent, 2, 1);
        expect_report("a: b\n  c: d\n", "UnexpectedToken", ContinuationAfterValue, 2, 3);
        expect_report("key:\nvalue\n", "UnexpectedToken", StrayValue, 2, 1);
        expect_report("- item1\n- item2\ninvalid: x\n", "UnexpectedToken", KeyBesideSequence, 3, 1);

        // One line holding more than one node.
        expect_report("a: b: c\n", "UnexpectedToken", NestedMappingOnLine, 1, 5);
        expect_report("key: \"v\" no key: nor value\n", "UnexpectedToken", TrailingContent, 1, 10);
        expect_report("key: - a\n", "UnexpectedToken", SequenceOnKeyLine, 1, 6);
        expect_report("--- a: b\n", "UnexpectedToken", BlockOnDocumentStartLine, 1, 5);

        // Scalars.
        expect_report("bad: \"abc\n", "UnclosedString", UnclosedString, 1, 6);
        expect_report("bad: 'abc\n", "UnclosedString", UnclosedString, 1, 6);
        expect_report("bad: \"ok \\c\"\n", "UnexpectedToken", InvalidEscape, 1, 10);
        expect_report("bad: \"\\uD800\"\n", "InvalidUnicodeEscape", InvalidUnicodeEscape, 1, 7);
        expect_report("a: |0\n  x\n", "InvalidBlockHeader", InvalidBlockHeader, 1, 4);

        // Properties, aliases, directives, documents.
        expect_report("a: 1\nb: *missing\n", "UndefinedAlias", UndefinedAlias, 2, 4);
        expect_report("!!str !!int x\n", "DuplicateProperty", DuplicateProperty, 1, 1);
        expect_report("--- !prefix!A\na: b\n", "UndefinedTagHandle", UndefinedTagHandle, 1, 5);
        expect_report("a: !<x y\n", "InvalidTag", InvalidTag, 1, 4);
        expect_report("a: &\n", "InvalidAnchor", InvalidAnchor, 1, 4);
        expect_report("a: *\n", "InvalidAlias", InvalidAlias, 1, 4);
        expect_report("%YAML 1.2 foo\n---\n", "InvalidDirective", InvalidDirective, 1, 1);
        expect_report("%YAML 1.2\na: 1\n", "InvalidDirective", MissingDocumentStart, 2, 1);
        expect_report("a: 1\n---\nb: 2\n", "MultipleDocuments", MultipleDocuments, 2, 1);
    }

    #[test]
    fn parse_with_report_underlines_the_offending_bytes_where_it_knows_them() {
        let mut report = Report::default();
        let src = b"bad: \"x \\q y\"\n";
        assert_eq!(err!(UnexpectedToken), parse_with_report(src, Type::V1_2_2, &mut report).unwrap_err());
        let d = report.diag.unwrap();
        assert_eq!(b"\\q", &src[d.offset..d.end.unwrap()]);

        let mut report = Report::default();
        let tag = b"a: 1\nb: !e!x 2\n";
        assert_eq!(err!(UndefinedTagHandle), parse_with_report(tag, Type::V1_2_2, &mut report).unwrap_err());
        let d = report.diag.unwrap();
        assert_eq!(b"!e!x", &tag[d.offset..d.end.unwrap()]);
    }

    #[test]
    fn parse_with_report_leaves_the_report_empty_on_a_clean_parse() {
        let mut report = Report::default();
        parse_with_report(b"a: [1, 2]\nb: {c: d}\n", Type::V1_2_2, &mut report).unwrap();
        assert!(report.diag.is_none());
    }

    #[test]
    fn tag_attaches_to_mapping_value_and_extends_span_left() {
        let doc = p("k: !!int 5\n");
        let value = val(&doc, &[k("k")]);
        assert_eq!(b"!!int", tag_text(&doc.ast, value.id).unwrap());
        assert_eq!(b"!!int 5", doc.span(&value).of(&doc.source));
        assert_eq!(b"!!int", doc.tag_span(&value).unwrap().of(&doc.source));
    }

    #[test]
    fn tag_on_root_scalar() {
        let doc = p("!!str foo\n");
        let r = root(&doc);
        assert_eq!(b"foo", r.kind.as_string().unwrap());
        assert_eq!(b"!!str", tag_text(&doc.ast, r.id).unwrap());
    }

    #[test]
    fn tag_bare_bang_is_a_tagged_null_root() {
        let doc = p("!\n");
        let r = root(&doc);
        assert_eq!(NodeTag::Null, r.kind.tag());
        assert_eq!(b"!", tag_text(&doc.ast, r.id).unwrap());
    }

    #[test]
    fn tag_empty_value_is_a_tagged_null() {
        let doc = p("t: !!str\n");
        let value = val(&doc, &[k("t")]);
        assert_eq!(NodeTag::Null, value.kind.tag());
        assert_eq!(b"!!str", tag_text(&doc.ast, value.id).unwrap());
    }

    #[test]
    fn tag_two_tags_on_one_node_is_rejected() {
        test_parser_error("!!str !!int x\n", "DuplicateProperty");
    }

    #[test]
    fn directive_yaml_before_document_is_consumed() {
        assert_eq!(b"foo", root(&p("%YAML 1.2\n---\nfoo\n")).kind.as_string().unwrap());
    }

    #[test]
    fn directive_tag_declares_a_handle_used_by_a_tag() {
        let doc = p("%TAG !e! tag:example.com,2000:app/\n---\n!e!foo bar\n");
        assert_eq!(b"bar", root(&doc).kind.as_string().unwrap());
        assert_eq!(b"!e!foo", tag_text(&doc.ast, doc.ast.root).unwrap());
    }

    #[test]
    fn directive_a_reserved_directive_is_ignored() {
        assert_eq!(b"x", root(&p("%FOO bar baz # ignored\n---\n\"x\"\n")).kind.as_string().unwrap());
    }

    #[test]
    fn directive_misuse_is_rejected() {
        test_parser_error("%YAML 1.2\n", "InvalidDirective");
        test_parser_error("%YAML 1.2\n...\n", "InvalidDirective");
        test_parser_error("%YAML 1.2\n%YAML 1.2\n---\n", "InvalidDirective");
        test_parser_error("%YAML 1.2 foo\n---\n", "InvalidDirective");
        test_parser_error("---\nkey: value\n%YAML 1.2\n---\n", "InvalidDirective");
        test_parser_error("--- !prefix!A\na: b\n", "UndefinedTagHandle");
    }

    #[test]
    fn directive_a_multiline_plain_scalar_may_absorb_a_percent_line_xlq9() {
        assert_eq!(b"scalar %YAML 1.2", root(&p("---\nscalar\n%YAML 1.2\n")).kind.as_string().unwrap());
    }

    #[test]
    fn tag_flow_tagged_null_value() {
        let doc = p("{a: !!str}\n");
        let a = val(&doc, &[k("a")]);
        assert_eq!(NodeTag::Null, a.kind.tag());
        assert_eq!(b"!!str", tag_text(&doc.ast, a.id).unwrap());
    }

    #[test]
    fn tag_flow_tagged_null_key() {
        let doc = p("{!!str : bar}\n");
        let kv = first_pair(&doc.ast, doc.ast.root);
        assert_eq!(NodeTag::Null, doc.ast.node(kv.key).kind.tag());
        assert_eq!(b"!!str", tag_text(&doc.ast, kv.key).unwrap());
    }

    #[test]
    fn tag_deferred_null_sequence_item_keeps_its_tag_and_the_next_item() {
        let doc = p("- !!str\n- y\n");
        let item0 = val(&doc, &[ix(0)]);
        let item1 = val(&doc, &[ix(1)]);
        assert_eq!(NodeTag::Null, item0.kind.tag());
        assert_eq!(b"!!str", tag_text(&doc.ast, item0.id).unwrap());
        assert_eq!(b"y", item1.kind.as_string().unwrap());
        assert!(doc.ast.tag_of(item1.id).is_none());
    }

    #[test]
    fn anchor_recorded_alias_resolves_to_it() {
        let doc = p("a: &x 1\nb: *x\n");
        let a = val(&doc, &[k("a")]);
        assert_eq!(b"x", doc.ast.anchor_of(a.id).unwrap());
        let b = val(&doc, &[k("b")]);
        assert_eq!(NodeKind::Alias(b"x"), b.kind);
        assert_eq!(a.id, doc.ast.resolve_alias(&b).unwrap());
    }

    #[test]
    fn anchor_on_a_collection() {
        let doc = p("a: &x [1, 2]\nb: *x\n");
        let a = val(&doc, &[k("a")]);
        assert_eq!(NodeTag::Sequence, a.kind.tag());
        assert_eq!(b"x", doc.ast.anchor_of(a.id).unwrap());
        assert_eq!(a.id, doc.ast.resolve_alias(&val(&doc, &[k("b")])).unwrap());
    }

    #[test]
    fn anchor_nearest_preceding_anchor_wins_on_redefinition() {
        let doc = p("- &x 1\n- &x 2\n- *x\n");
        assert_eq!(val(&doc, &[ix(1)]).id, doc.ast.resolve_alias(&val(&doc, &[ix(2)])).unwrap());
    }

    #[test]
    fn anchor_undefined_alias_is_rejected() {
        test_parser_error("p: *missing\n", "UndefinedAlias");
    }

    #[test]
    fn merge_pulls_keys_host_overrides_sequence_earlier_wins() {
        let doc = p("base: &b\n  x: 1\n  y: 2\nover: &o\n  x: 9\nderived:\n  <<: [*o, *b]\n  y: 3\n");
        let derived = val(&doc, &[k("derived")]);
        let y = doc.ast.merged_child(&derived, b"y").unwrap().unwrap();
        assert_eq!(b"3", doc.ast.node(y).kind.as_number().unwrap().raw);
        let x = doc.ast.merged_child(&derived, b"x").unwrap().unwrap();
        assert_eq!(b"9", doc.ast.node(x).kind.as_number().unwrap().raw);
        assert!(doc.ast.merged_child(&derived, b"nope").unwrap().is_none());
    }

    #[test]
    fn merge_cyclic_alias_does_not_hang() {
        let doc = p("a: &m\n  <<: *m\n  k: 1\n");
        let a = val(&doc, &[k("a")]);
        assert!(doc.ast.merged_child(&a, b"k").unwrap().is_some());
        assert_eq!(err!(AliasCycle), doc.ast.merged_child(&a, b"absent").unwrap_err());
    }

    #[test]
    fn captures_leading_trailing_comments_block_scalar_hash_stays_content() {
        let doc = p("# document header\nname: fig # the project\nnums:\n# first item\n- 1\n- 2 # second\ntext: |\n  # not a comment\n  body");
        let ast = &doc.ast;
        let first = root(&doc).kind.as_mapping().unwrap().unwrap();
        let kv_name = ast.node(first).kind.as_keyvalue().unwrap();
        assert_eq!(b"document header", ast.comments(kv_name.key).leading[0].text);
        assert_eq!(b"the project", ast.comments(kv_name.value).trailing.unwrap().text);

        let second = ast.node(first).next_sibling.unwrap();
        let kv_nums = ast.node(second).kind.as_keyvalue().unwrap();
        let item1 = ast.node(ast.node(kv_nums.value).kind.as_sequence().unwrap().unwrap());
        let item2 = ast.node(item1.next_sibling.unwrap());
        assert_eq!(b"first item", ast.comments(item1.id).leading[0].text);
        assert_eq!(b"second", ast.comments(item2.id).trailing.unwrap().text);

        let kv_text = ast.node(ast.node(second).next_sibling.unwrap()).kind.as_keyvalue().unwrap();
        let text = ast.node(kv_text.value).kind.as_string().unwrap();
        assert!(text.windows(15).any(|w| w == b"# not a comment"));
        assert!(ast.comments(kv_text.value).trailing.is_none());
    }

    #[test]
    fn comment_after_a_block_collection_value_leads_the_next_sibling() {
        let doc = p("steps:\n- uses: checkout\n  with:\n    version: one\n  # leads run\n- run: build");
        let ast = &doc.ast;
        let kv_steps = first_pair(ast, ast.root);
        let item1 = ast.node(ast.node(kv_steps.value).kind.as_sequence().unwrap().unwrap());
        let item2 = ast.node(item1.next_sibling.unwrap());
        let run_pair = first_pair(ast, item2.id);
        let run_leading = ast.comments(run_pair.key).leading;
        assert_eq!(1, run_leading.len());
        assert_eq!(b"leads run", run_leading[0].text);
        let uses_pair = first_pair(ast, item1.id);
        if let Some(wid) = ast.node(uses_pair.value).next_sibling {
            assert!(ast.comments(wid).leading.is_empty());
        }
    }

    #[test]
    fn comment_free_document_carries_no_comment_table() {
        assert!(!p("a: 1\nb: [2, 3]\n").ast.has_comments());
    }

    #[test]
    fn anchor_collection_and_its_first_key_both_anchored() {
        let doc = p("top: &node\n  &k key: one\n");
        let top = val(&doc, &[k("top")]);
        assert_eq!(NodeTag::Mapping, top.kind.tag());
        assert_eq!(b"node", doc.ast.anchor_of(top.id).unwrap());
    }

    #[test]
    fn anchor_next_line_value_must_out_indent_its_key() {
        let doc = p("seq:\n &a\n- x\n");
        let seq = val(&doc, &[k("seq")]);
        assert_eq!(NodeTag::Sequence, seq.kind.tag());
        assert_eq!(b"a", doc.ast.anchor_of(seq.id).unwrap());
        test_parser_error("seq:\n&a\n- x\n", "UnexpectedToken");
    }

    #[test]
    fn anchor_misplaced_property_is_rejected() {
        test_parser_error("&anchor - sequence entry\n", "UnexpectedToken");
        test_parser_error("- a\n&b\n- c\n", "UnexpectedToken");
    }

    #[test]
    fn a_block_collection_cannot_begin_on_the_marker_line() {
        test_parser_error("--- a: b\n", "UnexpectedToken");
        test_parser_error("--- &anchor a: b\n", "UnexpectedToken");
        test_parser_error("--- - x\n", "UnexpectedToken");
        assert_eq!(b"b", s(&p("---\na: b\n"), &[k("a")]));
    }

    #[test]
    fn plain_scalar_trailing_comment_on_a_continuation_line() {
        assert_eq!(b"plain value", s(&p("b: plain\n value  # comment\n"), &[k("b")]));
    }

    #[test]
    fn a_property_only_line_at_a_different_column_than_its_node() {
        let doc = p("folded:\n   !foo\n  >1\n value\n");
        let v = val(&doc, &[k("folded")]);
        assert_eq!(NodeTag::String, v.kind.tag());
        assert_eq!(b"!foo", tag_text(&doc.ast, v.id).unwrap());
    }

    #[test]
    fn block_scalar_next_line_header_with_explicit_indent_over_the_owner_column() {
        assert_eq!(NodeTag::String, tag(&p("folded:\n  >1\n value\n"), &[k("folded")]));
    }

    #[test]
    fn binary_is_a_byte_string_its_base64_unwrapped() {
        let doc = p("a: !!binary |\n  aGVs\n  bG8=\nb: !!binary not*base64\nc: !!binary \"\"\n");
        let a = val(&doc, &[k("a")]);
        let NodeKind::Extended(e) = a.kind else { panic!() };
        assert_eq!((ExtKind::Data, &b"aGVsbG8="[..]), (e.kind, e.text));
        assert_eq!(b"!!binary", tag_text(&doc.ast, a.id).unwrap());
        assert_eq!(b"not*base64", s(&doc, &[k("b")]));
        let NodeKind::Extended(c) = val(&doc, &[k("c")]).kind else { panic!() };
        assert_eq!((ExtKind::Data, &b""[..]), (c.kind, c.text));
    }

    #[test]
    fn binary_on_a_byte_string_asks_nothing_more_on_text_it_is_custom() {
        let data = NodeKind::Extended(Extended { kind: ExtKind::Data, text: b"aGk=" });
        assert_eq!(table::TAG_NON_SPECIFIC, resolve_tag(&[], b"!!binary", data).resolved);
        assert_eq!(table::TAG_CUSTOM, resolve_tag(&[], b"!!binary", NodeKind::String(b"x")).resolved);
    }

    #[test]
    fn tag_untagged_node_has_no_side_table_entry() {
        let doc = p("a: 1\n");
        let value = val(&doc, &[k("a")]);
        assert!(doc.ast.tag_of(value.id).is_none());
        assert!(doc.tag_span(&value).is_none());
    }

    fn n(id: NodeId, kind: NodeKind<'static>, next_sibling: Option<NodeId>) -> Node<'static> {
        Node { id, kind, next_sibling }
    }

    #[test]
    fn simple_yaml_document() {
        let nodes = [
            n(0, NodeKind::Sequence(Some(1)), None),
            n(1, NodeKind::Mapping(Some(4)), None),
            n(2, NodeKind::String(b"hello"), None),
            n(3, NodeKind::String(b"world"), None),
            n(4, NodeKind::KeyValue(KeyValue { key: 2, value: 3 }), None),
        ];
        assert!(Literal { root: 0, nodes: &nodes }.matches(&p("- hello: world").ast));
    }

    #[test]
    fn yaml_flat_mapping() {
        let nodes = [
            n(0, NodeKind::Mapping(Some(3)), None),
            n(1, NodeKind::String(b"name"), None),
            n(2, NodeKind::String(b"Ada"), None),
            n(3, NodeKind::KeyValue(KeyValue { key: 1, value: 2 }), Some(6)),
            n(4, NodeKind::String(b"age"), None),
            n(5, NodeKind::Number(Number { raw: b"37", kind: NumberKind::Integer }), None),
            n(6, NodeKind::KeyValue(KeyValue { key: 4, value: 5 }), None),
        ];
        assert!(Literal { root: 0, nodes: &nodes }.matches(&p("name: Ada\nage: 37\n").ast));
    }

    #[test]
    fn yaml_nested_mapping() {
        let nodes = [
            n(0, NodeKind::Mapping(Some(6)), None),
            n(1, NodeKind::String(b"root"), None),
            n(2, NodeKind::Mapping(Some(5)), None),
            n(3, NodeKind::String(b"child"), None),
            n(4, NodeKind::String(b"value"), None),
            n(5, NodeKind::KeyValue(KeyValue { key: 3, value: 4 }), None),
            n(6, NodeKind::KeyValue(KeyValue { key: 1, value: 2 }), Some(9)),
            n(7, NodeKind::String(b"next"), None),
            n(8, NodeKind::Boolean(true), None),
            n(9, NodeKind::KeyValue(KeyValue { key: 7, value: 8 }), None),
        ];
        assert!(Literal { root: 0, nodes: &nodes }.matches(&p("root:\n  child: value\nnext: true\n").ast));
    }

    #[test]
    fn sequence_item_mapping_continuation() {
        let doc = p("- adapter: CodeLLDB\n  label: Debug library tests\n  build:\n    command: zig\n    args:\n      - build\n      - install-tests\n");
        assert_eq!(b"Debug library tests", s(&doc, &[ix(0), k("label")]));
        assert_eq!(NodeTag::Mapping, tag(&doc, &[ix(0), k("build")]));
        assert_eq!(b"install-tests", s(&doc, &[ix(0), k("build"), k("args"), ix(1)]));
    }

    #[test]
    fn empty_flow_collection_scalars() {
        let doc = p("env: {}\ntags: []\n");
        assert_eq!(NodeKind::Mapping(None), val(&doc, &[k("env")]).kind);
        assert_eq!(NodeKind::Sequence(None), val(&doc, &[k("tags")]).kind);
    }

    #[test]
    fn decodes_single_quoted_scalars() {
        let doc = p("single: 'here''s to \"quotes\"'\npath: 'C:\\Temp'\n\"quoted:key\": 'value # not comment'\n");
        assert_eq!(b"here's to \"quotes\"", s(&doc, &[k("single")]));
        assert_eq!(b"C:\\Temp", s(&doc, &[k("path")]));
        assert_eq!(b"value # not comment", s(&doc, &[k("quoted:key")]));
    }

    #[test]
    fn decodes_double_quoted_scalars() {
        let doc = p("double: \"quote: \\\" slash: \\\\ newline: \\n tab: \\t zero: \\0 hex: \\x41 unicode: \\u00E9 big: \\U0001D11E\"\nnumber: \"37\"\nplain: 37\n");
        assert_eq!(&b"quote: \" slash: \\ newline: \n tab: \t zero: \x00 hex: A unicode: \xC3\xA9 big: \xF0\x9D\x84\x9E"[..], s(&doc, &[k("double")]));
        assert_eq!(b"37", s(&doc, &[k("number")]));
        assert_eq!(NodeTag::Number, tag(&doc, &[k("plain")]));
    }

    #[test]
    fn rejects_invalid_double_quoted_escapes() {
        test_parser_error("bad: \"\\c\"\n", "UnexpectedToken");
        test_parser_error("bad: \"\\xq0\"\n", "InvalidUnicodeEscape");
        test_parser_error("bad: \"\\uD800\"\n", "InvalidUnicodeEscape");
    }

    #[test]
    fn core_schema_type_resolution() {
        let doc = p("n1: null\nn2: NULL\nn3: ~\nb1: True\nb2: FALSE\ni1: 42\ni2: -7\ni3: +5\ni4: 0x1A\ni5: 0o17\nf1: 3.14\nf2: 1e3\nf3: -.inf\nf4: .nan\ns1: yes\ns2: 1_000\ns3: 0x\n");
        for key in ["n1", "n2", "n3"] {
            assert_eq!(NodeTag::Null, tag(&doc, &[k(key)]));
        }
        assert_eq!(NodeKind::Boolean(true), val(&doc, &[k("b1")]).kind);
        assert_eq!(NodeKind::Boolean(false), val(&doc, &[k("b2")]).kind);
        let cases = [
            ("i1", NumberKind::Integer, "42"),
            ("i2", NumberKind::Integer, "-7"),
            ("i3", NumberKind::Integer, "+5"),
            ("i4", NumberKind::Integer, "0x1A"),
            ("i5", NumberKind::Integer, "0o17"),
            ("f1", NumberKind::Float, "3.14"),
            ("f2", NumberKind::Float, "1e3"),
            ("f3", NumberKind::Float, "-.inf"),
            ("f4", NumberKind::Float, ".nan"),
        ];
        for (key, kind, r) in cases {
            let number = val(&doc, &[k(key)]).kind.as_number().unwrap();
            assert_eq!((r.as_bytes(), kind), (number.raw, number.kind));
        }
        // Things that look number-ish but are not core-schema numbers stay strings.
        for key in ["s1", "s2", "s3"] {
            assert_eq!(NodeTag::String, tag(&doc, &[k(key)]));
        }
    }

    #[test]
    fn reads_a_leading_bom_as_no_part_of_the_first_line() {
        let doc = p("\u{FEFF}a:\n  b: |\n    t\n");
        assert_eq!(b"t\n", s(&doc, &[k("a"), k("b")]));
        assert_eq!(3, doc.span(&root(&doc)).start);
    }

    #[test]
    fn root_scalar_documents() {
        assert_eq!(b"hello world", root(&p("hello world\n")).kind.as_string().unwrap());
        assert_eq!(NodeTag::Number, root(&p("42\n")).kind.tag());
        assert_eq!(b"quoted", root(&p("\"quoted\"\n")).kind.as_string().unwrap());
    }

    #[test]
    fn empty_documents_have_a_null_root() {
        for input in ["", "# just a comment\n", "\n\n", "...\n", "---\n"] {
            assert_eq!(NodeTag::Null, root(&p(input)).kind.tag());
        }
    }

    #[test]
    fn block_context_empty_keys() {
        let doc = p(": a\n: b\n");
        let kv = first_pair(&doc.ast, doc.ast.root);
        assert_eq!(NodeTag::Null, doc.ast.node(kv.key).kind.tag());
        assert_eq!(b"a", doc.ast.node(kv.value).kind.as_string().unwrap());
        assert_eq!(NodeTag::Mapping, root(&p(":\n")).kind.tag());
        assert_eq!(NodeTag::Mapping, tag(&p("- :\n"), &[ix(0)]));
    }

    #[test]
    fn indented_comment_does_not_affect_structure() {
        let doc = p("a: 1\n  # indented comment\nb: 2\n");
        assert_eq!(NodeTag::Number, tag(&doc, &[k("a")]));
        assert_eq!(NodeTag::Number, tag(&doc, &[k("b")]));
    }

    #[test]
    fn document_markers_wrap_a_single_document() {
        assert_eq!(b"Ada", s(&p("---\nname: Ada\n...\n"), &[k("name")]));
    }

    #[test]
    fn rejects_multiple_documents() {
        test_parser_error("---\na: 1\n---\nb: 2\n", "MultipleDocuments");
        test_parser_error("a: 1\n...\nb: 2\n", "MultipleDocuments");
    }

    #[test]
    fn colon_inside_a_value_stays_in_the_scalar() {
        let doc = p("url: http://example.com\ntime: 12:30:00\n");
        assert_eq!(b"http://example.com", s(&doc, &[k("url")]));
        assert_eq!(b"12:30:00", s(&doc, &[k("time")]));
    }

    #[test]
    fn flow_sequence_of_scalars() {
        let doc = p("tags: [a, b, 3]\n");
        assert_eq!(NodeTag::Sequence, tag(&doc, &[k("tags")]));
        assert_eq!(b"a", s(&doc, &[k("tags"), ix(0)]));
        assert_eq!(b"b", s(&doc, &[k("tags"), ix(1)]));
        assert_eq!(NodeTag::Number, tag(&doc, &[k("tags"), ix(2)]));
    }

    #[test]
    fn flow_mapping_with_quoted_keys_and_nested_flow() {
        let doc = p("m: {a: 1, \"b c\": [x, y], d}\n");
        assert_eq!(NodeTag::Number, tag(&doc, &[k("m"), k("a")]));
        assert_eq!(NodeTag::Sequence, tag(&doc, &[k("m"), k("b c")]));
        assert_eq!(b"y", s(&doc, &[k("m"), k("b c"), ix(1)]));
        // A bare key in a flow mapping has an implicit null value.
        assert_eq!(NodeTag::Null, tag(&doc, &[k("m"), k("d")]));
    }

    #[test]
    fn flow_collection_spanning_multiple_lines() {
        assert_eq!(b"3", raw(&p("matrix: [\n  [1, 2],\n  [3, 4],\n]\n"), &[k("matrix"), ix(1), ix(0)]));
    }

    #[test]
    fn rejects_malformed_flow() {
        test_parser_error("x: [a, b\n", "UnexpectedToken");
        test_parser_error("x: [a]]\n", "UnexpectedToken");
        test_parser_error("flow: [a,\nb]\n", "InvalidIndent");
        test_parser_error("x: [-]\n", "UnexpectedToken");
    }

    #[test]
    fn tabs_as_separation_produce_clean_values() {
        let doc = p("key:\tvalue\nflag:\ttrue\n");
        assert_eq!(b"value", s(&doc, &[k("key")]));
        assert_eq!(NodeKind::Boolean(true), val(&doc, &[k("flag")]).kind);
    }

    #[test]
    fn multi_line_plain_scalar_folds_into_a_value() {
        let doc = p("desc: this is\n  a long\n  value\nnext: x\n");
        assert_eq!(b"this is a long value", s(&doc, &[k("desc")]));
        assert_eq!(b"x", s(&doc, &[k("next")]));
    }

    #[test]
    fn multi_line_plain_scalar_blank_line_becomes_newline() {
        assert_eq!(b"one two\nthree", s(&p("desc: one\n  two\n\n  three\n"), &[k("desc")]));
    }

    #[test]
    fn crlf_root_scalar_drops_the_trailing_cr() {
        assert_eq!(b"Root", root(&p("Root\r\n")).kind.as_string().unwrap());
    }

    #[test]
    fn crlf_multi_line_plain_scalar_folds_without_a_stray_cr() {
        assert_eq!(b"one two", s(&p("desc: one\r\n  two\r\n"), &[k("desc")]));
    }

    #[test]
    fn multi_line_plain_scalar_as_sequence_entry() {
        let doc = p("- one\n  two\n- three\n");
        assert_eq!(b"one two", s(&doc, &[ix(0)]));
        assert_eq!(b"three", s(&doc, &[ix(1)]));
    }

    #[test]
    fn plain_value_is_not_folded_across_a_mapping_key() {
        test_parser_error("a: b\n  c: d\n", "UnexpectedToken");
    }

    #[test]
    fn multi_line_double_quoted_scalar_folds_breaks() {
        assert_eq!(b"one two\nthree", s(&p("msg: \"one\n  two\n\n  three\"\n"), &[k("msg")]));
    }

    #[test]
    fn crlf_double_quoted_scalar_folds_a_blank_line_without_a_stray_cr() {
        assert_eq!(b"one two\nthree", s(&p("msg: \"one\r\n  two\r\n\r\n  three\"\r\n"), &[k("msg")]));
    }

    #[test]
    fn multi_line_single_quoted_scalar_folds_breaks() {
        assert_eq!(b"it's here", s(&p("msg: 'it''s\n  here'\n"), &[k("msg")]));
    }

    #[test]
    fn double_quoted_escaped_line_break_joins_directly() {
        assert_eq!(b"ab", s(&p("v: \"a\\\n  b\"\n"), &[k("v")]));
    }

    #[test]
    fn multi_line_quoted_scalar_as_value_not_key() {
        assert_eq!(b"a b", s(&p("k: \"a\n  b\"\n"), &[k("k")]));
        test_parser_error("\"a\n b\": 1\n", "UnexpectedToken");
    }

    #[test]
    fn get_val_by_path_chains_through_nested_mappings_and_sequences() {
        let doc = p("outer:\n  items:\n    - first\n    - second\n  meta:\n    name: ada\n");
        assert_eq!(b"second", s(&doc, &[k("outer"), k("items"), ix(1)]));
        assert_eq!(b"ada", s(&doc, &[k("outer"), k("meta"), k("name")]));
    }

    #[test]
    fn literal_block_scalar_preserves_line_breaks() {
        let doc = p("desc: |\n  line one\n  line two\nnext: x\n");
        assert_eq!(b"line one\nline two\n", s(&doc, &[k("desc")]));
        assert_eq!(b"x", s(&doc, &[k("next")]));
    }

    #[test]
    fn crlf_literal_block_scalar_strips_cr_from_each_line() {
        assert_eq!(b"line one\nline two\n", s(&p("desc: |\r\n  line one\r\n  line two\r\n"), &[k("desc")]));
    }

    #[test]
    fn folded_block_scalar_folds_line_breaks() {
        assert_eq!(b"one two three\nafter blank\n", s(&p("desc: >\n  one two\n  three\n\n  after blank\n"), &[k("desc")]));
    }

    #[test]
    fn block_scalar_chomping_indicators() {
        assert_eq!(b"a\n", s(&p("v: |\n  a\n\n\n"), &[k("v")]));
        assert_eq!(b"a", s(&p("v: |-\n  a\n\n\n"), &[k("v")]));
        assert_eq!(b"a\n\n\n", s(&p("v: |+\n  a\n\n\n"), &[k("v")]));
    }

    #[test]
    fn block_scalar_explicit_indent_indicator_preserves_leading_spaces() {
        assert_eq!(b"  indented\nflush\n", s(&p("v: |2\n    indented\n  flush\n"), &[k("v")]));
    }

    #[test]
    fn block_scalar_as_sequence_entry_and_nested_value() {
        let doc = p("steps:\n  - |\n    do a\n    do b\n  - run\n");
        assert_eq!(b"do a\ndo b\n", s(&doc, &[k("steps"), ix(0)]));
        assert_eq!(b"run", s(&doc, &[k("steps"), ix(1)]));
    }

    #[test]
    fn empty_block_scalar_is_an_empty_string() {
        let doc = p("a: |\nb: 1\n");
        assert_eq!(b"", s(&doc, &[k("a")]));
        assert_eq!(NodeTag::Number, tag(&doc, &[k("b")]));
    }

    #[test]
    fn root_block_scalar_document() {
        assert_eq!(b"hello\nworld\n", root(&p("|\n  hello\n  world\n")).kind.as_string().unwrap());
    }

    #[test]
    fn root_block_scalar_with_column_0_content() {
        assert_eq!(b"line1 line2 line3\n", root(&p("--- >\nline1\nline2\nline3\n")).kind.as_string().unwrap());
        assert_eq!(b"line1\n# kept\nline3\n", root(&p("--- |\nline1\n# kept\nline3\n")).kind.as_string().unwrap());
    }

    #[test]
    fn next_line_plain_value_folds_same_indent_continuations() {
        let doc = p("plain:\n  one two\n  three\nnext: x\n");
        assert_eq!(b"one two three", s(&doc, &[k("plain")]));
        assert_eq!(b"x", s(&doc, &[k("next")]));
    }

    #[test]
    fn explicit_block_keys() {
        let doc = p("? key\n: value\nplain: x\n");
        assert_eq!(b"value", s(&doc, &[k("key")]));
        assert_eq!(b"x", s(&doc, &[k("plain")]));
    }

    #[test]
    fn explicit_key_with_empty_value_is_null() {
        let doc = p("? a\n? b\nc:\n");
        for key in ["a", "b", "c"] {
            assert_eq!(NodeTag::Null, tag(&doc, &[k(key)]));
        }
    }

    #[test]
    fn mapping_mixes_explicit_and_implicit_entries() {
        let doc = p("mapping:\n  ? sky\n  : blue\n  sea: green\n");
        assert_eq!(b"blue", s(&doc, &[k("mapping"), k("sky")]));
        assert_eq!(b"green", s(&doc, &[k("mapping"), k("sea")]));
    }

    #[test]
    fn explicit_block_scalar_key() {
        assert_eq!(b"value", s(&p("? |\n  block key\n: value\n"), &[k("block key\n")]));
    }

    #[test]
    fn flow_pairs_in_flow_sequences() {
        let doc = p("[ a: 1, b: 2 ]\n");
        assert_eq!(NodeTag::Number, tag(&doc, &[ix(0), k("a")]));
        assert_eq!(NodeTag::Number, tag(&doc, &[ix(1), k("b")]));
        let doc = p("[ ? k : v, plain ]\n");
        assert_eq!(b"v", s(&doc, &[ix(0), k("k")]));
        assert_eq!(b"plain", s(&doc, &[ix(1)]));
    }

    #[test]
    fn rejects_implicit_flow_pair_with_colon_on_next_line() {
        test_parser_error("[ key\n  : value ]\n", "UnexpectedToken");
    }

    #[test]
    fn flow_json_style_adjacent_colons() {
        let doc = p("{ \"a\":1, \"b\": 2 }\n");
        assert_eq!(b"1", raw(&doc, &[k("a")]));
        assert_eq!(b"2", raw(&doc, &[k("b")]));
        assert_eq!(b"bar", s(&p("{ \"foo\"\n  :bar }\n"), &[k("foo")]));
        assert_eq!(b"ab:cd", s(&p("[ ab:cd ]\n"), &[ix(0)]));
    }

    #[test]
    fn flow_collection_as_a_block_mapping_key() {
        let doc = p("[a, b]: value\n");
        let kv = first_pair(&doc.ast, doc.ast.root);
        assert_eq!(NodeTag::Sequence, doc.ast.node(kv.key).kind.tag());
        assert_eq!(b"value", doc.ast.node(kv.value).kind.as_string().unwrap());
        test_parser_error("[1\n]: x\n", "UnexpectedToken");
    }

    #[test]
    fn multi_line_plain_flow_scalar_folds() {
        let doc = p("[ a\n  b, c ]\n");
        assert_eq!(b"a b", s(&doc, &[ix(0)]));
        assert_eq!(b"c", s(&doc, &[ix(1)]));
        assert_eq!(b"value", s(&p("{ multi\n  line: value }\n"), &[k("multi line")]));
    }

    #[test]
    fn multi_line_quoted_flow_scalar_folds() {
        let doc = p("[ \"a\n  b\", 'c\n  d' ]\n");
        assert_eq!(b"a b", s(&doc, &[ix(0)]));
        assert_eq!(b"c d", s(&doc, &[ix(1)]));
    }

    #[test]
    fn rejects_comment_inside_a_flow_plain_scalar() {
        test_parser_error("[ word1\n  # xxx\n  word2 ]\n", "UnexpectedToken");
    }

    #[test]
    fn rejects_a_flow_plain_scalar_starting_with_an_indicator_plus_space() {
        for src in ["a: {b: - a}\n", "a: [- x]\n", "a: {b: 1, c: - q}\n", "a: [1, - q]\n", "a: {b: ? k}\n"] {
            test_parser_error(src, "UnexpectedToken");
        }
    }

    #[test]
    fn flow_plain_scalars_may_still_start_with_an_indicator_before_non_space() {
        let doc = p("a: [-1, -x, '- q', x - y]\n");
        assert_eq!(b"-1", raw(&doc, &[k("a"), ix(0)]));
        assert_eq!(b"-x", s(&doc, &[k("a"), ix(1)]));
        assert_eq!(b"- q", s(&doc, &[k("a"), ix(2)]));
        assert_eq!(b"x - y", s(&doc, &[k("a"), ix(3)]));
    }

    #[test]
    fn flow_mapping_with_explicit_keys_and_empty_values() {
        assert_eq!(NodeTag::Null, tag(&p("{ ? foo :, : bar, }\n"), &[k("foo")]));
    }

    #[test]
    fn next_line_scalar_value() {
        let doc = p("key:\n  value\nnext: x\n");
        assert_eq!(b"value", s(&doc, &[k("key")]));
        assert_eq!(b"x", s(&doc, &[k("next")]));
    }

    #[test]
    fn next_line_scalar_value_nested() {
        let doc = p("a:\n  b:\n    c\n  d: e\n");
        assert_eq!(b"c", s(&doc, &[k("a"), k("b")]));
        assert_eq!(b"e", s(&doc, &[k("a"), k("d")]));
    }

    #[test]
    fn next_line_flow_value() {
        let doc = p("key:\n  [1, 2]\n");
        assert_eq!(NodeTag::Sequence, tag(&doc, &[k("key")]));
        assert_eq!(b"2", raw(&doc, &[k("key"), ix(1)]));
    }

    #[test]
    fn rejects_content_at_a_next_line_values_indent() {
        test_parser_error("key:\n  word1 word2\n  no: key\n", "UnexpectedToken");
        test_parser_error("key:\nvalue\n", "UnexpectedToken");
    }

    #[test]
    fn explicit_value_compact_block_sequence() {
        let doc = p("? k\n: - one\n  - two\n");
        assert_eq!(NodeTag::Sequence, tag(&doc, &[k("k")]));
        assert_eq!(b"one", s(&doc, &[k("k"), ix(0)]));
        assert_eq!(b"two", s(&doc, &[k("k"), ix(1)]));
    }

    #[test]
    fn rejects_compact_block_sequence_as_implicit_mapping_value() {
        test_parser_error("key: - a\n     - b\n", "UnexpectedToken");
    }

    #[test]
    fn sequence_entry_with_explicit_key_mapping_value() {
        assert_eq!(NodeTag::Mapping, tag(&p("- ? : x\n"), &[ix(0)]));
    }

    #[test]
    fn complex_block_keys_block_sequence_as_key() {
        let doc = p("? - a\n  - b\n: c\n");
        let kv = first_pair(&doc.ast, doc.ast.root);
        let key = doc.ast.node(kv.key);
        assert_eq!(NodeTag::Sequence, key.kind.tag());
        assert_eq!(b"a", doc.ast.node(key.kind.as_sequence().unwrap().unwrap()).kind.as_string().unwrap());
        assert_eq!(b"c", doc.ast.node(kv.value).kind.as_string().unwrap());
        assert_eq!(NodeTag::Mapping, tag(&p("complex:\n  ? - a\n  : b\n"), &[k("complex")]));
    }

    #[test]
    fn rejects_tab_before_a_block_sequence_indicator() {
        test_parser_error("?\t-\n", "TabIndent");
        test_parser_error("? -\n:\t-\n", "TabIndent");
    }

    #[test]
    fn compact_nested_sequences_keep_their_nesting() {
        expect_shape("- - s1\n  - s2\n- s3\n", "[ [ S S ] S ]");
        expect_shape("- - - x\n- - - y\n", "[ [ [ S ] ] [ [ S ] ] ]");
    }

    #[test]
    fn a_line_under_compact_nested_items_belongs_to_the_item_at_its_column() {
        let cases = [
            ("- - - a\n  - d\n", "[ [ [ S ] S ] ]"),
            ("- - - a\n    - c\n  - d\n", "[ [ [ S S ] S ] ]"),
            ("- - - a\n    - b\n  - d\n- e\n", "[ [ [ S S ] S ] S ]"),
            ("- - - a\n  - - b\n    - c\n", "[ [ [ S ] [ S S ] ] ]"),
            ("- - - - a\n      - b\n    - c\n  - d\n- e\n", "[ [ [ [ S S ] S ] S ] S ]"),
            ("- - k: v\n  - c\n", "[ [ { S S } S ] ]"),
            ("- - k: v\n  - c\n- d\n", "[ [ { S S } S ] S ]"),
            ("- - k: v\n    j: w\n  - c\n", "[ [ { S S S S } S ] ]"),
            ("- - k: v\n    j: w\n- e\n", "[ [ { S S S S } ] S ]"),
            ("- - - k: v\n    - c\n- d\n", "[ [ [ { S S } S ] ] S ]"),
            ("- - - k: v\n      j: w\n    - c\n  - d\n", "[ [ [ { S S S S } S ] S ] ]"),
            ("- - ? a\n    : b\n  - c\n", "[ [ { S S } S ] ]"),
            ("- - k:\n      x: 1\n  - c\n", "[ [ { S { S S } } S ] ]"),
            ("- - k:\n    - x\n  - c\n", "[ [ { S [ S ] } S ] ]"),
            ("- - k: v\n    j:\n      - z\n  - c\n", "[ [ { S S S [ S ] } S ] ]"),
            ("- k:\n  - - a\n    - b\n  - c\n  j: 1\n", "[ { S [ [ S S ] S ] S S } ]"),
            ("- k: v\n  j:\n  - - a\n    - b\n", "[ { S S S [ [ S S ] ] } ]"),
            ("- - - a\n\n    # note\n    - b\n  # other\n\n  - d\n", "[ [ [ S S ] S ] ]"),
            ("- - - a # one\n    - b # two\n  - d # three\n", "[ [ [ S S ] S ] ]"),
            ("- - |\n    text\n  - d\n", "[ [ S S ] ]"),
            ("- - - |\n      text\n    - c\n  - d\n", "[ [ [ S S ] S ] ]"),
            ("- - >\n    folded\n    text\n  - d\n- e\n", "[ [ S S ] S ]"),
            ("- - k: |\n      text\n    j: w\n  - c\n", "[ [ { S S S S } S ] ]"),
        ];
        for (src, want) in cases {
            expect_shape(src, want);
        }
        // A column between two nested items begins nothing.
        test_parser_error("- - - a\n   - d\n", "UnexpectedToken");
        test_parser_error("- - a\n - d\n", "UnexpectedToken");
    }

    #[test]
    fn compact_nested_items_keep_their_values() {
        let cases: [(&str, &[PathSegment<'_>], &str); 5] = [
            ("- - - a\n  - d\n", &[ix(0), ix(1)], "d"),
            ("- - - a\n    - c\n  - d\n", &[ix(0), ix(1)], "d"),
            ("- - - a\n    - b\n  - d\n- e\n", &[ix(1)], "e"),
            ("- - k: v\n  - c\n", &[ix(0), ix(1)], "c"),
            ("- - k: v\n    j: w\n  - c\n", &[ix(0), ix(1)], "c"),
        ];
        for (src, path, want) in cases {
            assert_eq!(want.as_bytes(), s(&p(src), path), "{src:?}");
        }
    }

    #[test]
    fn a_plain_scalar_continues_onto_a_more_indented_line_that_starts_with_an_indicator() {
        const K: PathSegment<'static> = PathSegment::Key(b"k");
        const J: PathSegment<'static> = PathSegment::Key(b"j");
        const X: PathSegment<'static> = PathSegment::Key(b"x");
        const I0: PathSegment<'static> = PathSegment::Index(0);
        const I1: PathSegment<'static> = PathSegment::Index(1);
        let cases: &[(&str, &[PathSegment<'_>], &str)] = &[
            ("- a\n  - b\n", &[I0], "a - b"),
            ("- a\n - b\n", &[I0], "a - b"),
            ("- a\n    - b\n", &[I0], "a - b"),
            ("- a\n  ? b\n", &[I0], "a ? b"),
            ("- a\n  -\n", &[I0], "a -"),
            ("- a\n  -  b\n", &[I0], "a -  b"),
            ("- a\n  - - b\n", &[I0], "a - - b"),
            ("- a\n  - b\n  - c\n", &[I0], "a - b - c"),
            ("- a\n  - b\n- c\n", &[I1], "c"),
            ("- a\n  b\n  - c\n", &[I0], "a b - c"),
            ("- a\n  - b # c\n", &[I0], "a - b"),
            ("- a\n  - b\n  # c\n- d\n", &[I0], "a - b"),
            ("- &x a\n  - b\n", &[I0], "a - b"),
            ("- !!str a\n  - b\n", &[I0], "a - b"),
            // Blank lines between fold to line breaks.
            ("- a\n\n  - b\n", &[I0], "a\n- b"),
            ("- a\n\n\n  - b\n- c\n", &[I0], "a\n\n- b"),
            // Under a key: the floor is the key's column, not the line's.
            ("k: a\n  - b\n", &[K], "a - b"),
            ("k: a\n - b\n", &[K], "a - b"),
            ("k: a\n  ? b\n", &[K], "a ? b"),
            ("k: a\n  -\n", &[K], "a -"),
            ("k: a\n  - b\nj: c\n", &[K], "a - b"),
            ("k: a\n  - b # c\nj: d\n", &[J], "d"),
            ("- k: a\n   - b\n", &[I0, K], "a - b"),
            ("- &a k: v\n   - b\n", &[I0, K], "v - b"),
            ("- k: a\n\n    - b\n", &[I0, K], "a\n- b"),
            ("- k: a\n      - b\n  j: c\n", &[I0, J], "c"),
            // A value on the line below its key or dash continues past the same floor.
            ("k:\n  a\n  - b\n", &[K], "a - b"),
            ("k:\n    a\n  - b\n", &[K], "a - b"),
            ("k:\n  a\n b\n", &[K], "a b"),
            ("- k:\n    a\n   - b\n", &[I0, K], "a - b"),
            ("-\n  a\n  - b\n", &[I0], "a - b"),
            ("k:\n- a\n  - b\n", &[K, I0], "a - b"),
            ("k:\n  - a\n    - b\n  - c\n", &[K, I0], "a - b"),
            ("? x\n: a\n  - b\n", &[X], "a - b"),
            // Compact nested items: the floor is the innermost dash.
            ("- - a\n    - b\n", &[I0, I0], "a - b"),
            ("- - a\n   - b\n", &[I0, I0], "a - b"),
            ("- - a\n    ? b\n", &[I0, I0], "a ? b"),
            ("- - - a\n      - b\n", &[I0, I0, I0], "a - b"),
            ("- - a\n    - b\n  - c\n", &[I0, I1], "c"),
            // The document's root continues onto every line.
            ("a\n- b\n", &[], "a - b"),
            ("a\n  - b\n", &[], "a - b"),
            ("&x a\n- b\n", &[], "a - b"),
            ("--- a\n- b\n", &[], "a - b"),
        ];
        for (src, path, want) in cases {
            assert_eq!(want.as_bytes(), s(&p(src), path), "{src:?}");
        }
        // A `? ` line under an explicit key is part of the key; the dash at the
        // item column is the next item, as before.
        expect_shape("? a\n  - b\n: c\n", "{ S S }");
        expect_shape("- ? a\n    - b\n", "[ { S S } ]");
        expect_shape("? - a\n    - b\n", "{ [ S ] S }");
        expect_shape("- - a\n  - b\n", "[ [ S S ] ]");
        // Where the line opens a node, the dash still begins a sequence.
        expect_shape("k:\n  - b\n", "{ S [ S ] }");
        expect_shape("k:\n- b\n", "{ S [ S ] }");
        expect_shape("-\n  - b\n", "[ [ S ] ]");
        // A flow collection is no plain scalar of the block.
        expect_shape("- [a\n  - b]\n", "[ [ S ] ]");
    }

    #[test]
    fn a_line_indented_under_an_entry_whose_node_is_whole_is_refused() {
        for src in [
            "- a\n  : b\n",
            "- a\n  : b\n  c\n",
            "- a\n  b: c\n",
            "- a\n  - b: c\n",
            "- a\n  - b:\n",
            "- a\n  - b\n  - c: d\n",
            "- a\n  ? b\n  : c\n",
            "- a\n  # c\n  - b\n",
            "- a # x\n  - b\n",
            "- \"a\"\n  - b\n",
            "- 'a'\n  - b\n",
            "? a\n  : b\n",
            "? a\n  b: c\n",
            "- k: a\n  b\n",
            "- k: a\n  - b\n",
            "k: a\n  : b\n",
            "k: a\n- b\n",
            "k: \"a\"\n  - b\n",
            "k: a # x\n  - b\n",
            "k: [a]\n  - b\n",
            "k: a\n  - b\n  : c\n",
            "- a\n  \t- b\n",
            "k: a\n  \t- b\n",
        ] {
            assert!(parse(src.as_bytes(), Type::V1_2_2).is_err(), "accepted: {src:?}");
        }
        expect_report("- \"a\"\n  - b\n", "UnexpectedToken", Code::ContinuationAfterValue, 2, 3);
        expect_report("- a\n  b: c\n", "UnexpectedToken", Code::ContinuationAfterValue, 2, 3);
    }

    #[test]
    fn key_after_a_nested_block_sequence_is_a_sibling_not_absorbed() {
        expect_shape("a:\n  items:\n  - k: 1\n  - k: 2\nb: 2\n", "{ S { S [ { S S } { S S } ] } S S }");
        expect_shape("a:\n  items:\n    - k: 1\n    - k: 2\nb: 2\n", "{ S { S [ { S S } { S S } ] } S S }");
        expect_shape("root:\n  a:\n    items:\n    - k: 1\n  b: 2\ntop: 9\n", "{ S { S { S [ { S S } ] } S S } S S }");
        expect_shape("a:\n  items:\n  - 1\n  - 2\nb: 2\n", "{ S { S [ S S ] } S S }");
        expect_shape("aud:\n- name: x\n  pub: true\nm: true\n", "{ S [ { S S S S } ] S S }");
    }

    #[test]
    fn block_value_on_a_compact_sequence_item_key_nests_under_that_key() {
        expect_shape("- outer:\n    inner: 1\n", "[ { S { S S } } ]");
        expect_shape("- k:\n    - x\n    - y\n", "[ { S [ S S ] } ]");
        expect_shape("- outer:\n    inner: 1\n- second: 2\n", "[ { S { S S } } { S S } ]");
        expect_shape("- a: 1\n  b: 2\n", "[ { S S S S } ]");
        expect_shape("- a: 1\n  b:\n    c: 2\n", "[ { S S S { S S } } ]");
    }

    #[test]
    fn sibling_key_after_a_first_keys_block_value_returns_to_the_item_column() {
        expect_shape("- outer:\n    inner: 1\n  sib: 2\n", "[ { S { S S } S S } ]");
        expect_shape("- outer:\n    inner: 1\n  sib: 2\n- second: 3\n", "[ { S { S S } S S } { S S } ]");
        expect_shape("- outer:\n    inner: 1\n  sib:\n    deep: 2\n  third: 3\n", "[ { S { S S } S { S S } S S } ]");
        test_parser_error("- outer:\n    inner: 1\n   sib: 2\n", "InvalidIndent");
    }

    #[test]
    fn complex_explicit_keys_take_collections_as_the_key() {
        expect_shape("c1:\n  ? - a\nc2:\n  ? - a\n  : b\n", "{ S { [ S ] S } S { [ S ] S } }");
        expect_shape("?\n- a\n- b\n:\n- c\n- d\n", "{ [ S S ] [ S S ] }");
        expect_shape("- ? : x\n", "[ { { S S } S } ]");
        expect_shape("? []: x\n", "{ { [ ] S } S }");
    }

    #[test]
    fn merge_key_is_an_ordinary_string_key() {
        let doc = p("<<: {a: 1}\nb: 2\n");
        assert_eq!(NodeTag::Mapping, tag(&doc, &[k("<<")]));
        assert_eq!(NodeTag::Number, tag(&doc, &[k("b")]));
    }

    #[test]
    fn rejects_flow_shaped_root_scalars() {
        test_parser_error("[ a, b, c ] ]\n", "UnexpectedToken");
        test_parser_error("]\n", "UnexpectedToken");
        test_parser_error(",x\n", "UnexpectedToken");
    }

    #[test]
    fn rejects_malformed_block_scalar_indentation_indicator() {
        test_parser_error("--- |0\n", "InvalidBlockHeader");
        test_parser_error("--- |10\n", "InvalidBlockHeader");
        p("--- |1-\n");
    }

    #[test]
    fn explicit_block_indent_counts_from_the_owner_column() {
        let doc = p("- k: |2\n     X\n    y: 1\n  m: 2\n");
        assert_eq!(b" X\ny: 1\n", s(&doc, &[ix(0), k("k")]));
        assert_eq!(NodeTag::Number, tag(&doc, &[ix(0), k("m")]));
    }

    #[test]
    fn rejects_an_inline_mapping_value() {
        test_parser_error("a: b: c: d\n", "UnexpectedToken");
        test_parser_error("---\na: 'b': c\n", "UnexpectedToken");
    }

    #[test]
    fn rejects_trailing_content_after_a_complete_value() {
        test_parser_error("key: \"v\" no key: nor value\n", "UnexpectedToken");
        test_parser_error("---\nx: { y: z }in: valid\n", "UnexpectedToken");
    }

    #[test]
    fn rejects_a_mapping_key_at_a_sequences_indent() {
        test_parser_error("- item1\n- item2\ninvalid: x\n", "UnexpectedToken");
    }

    #[test]
    fn indentless_sequence_returns_to_the_enclosing_mapping() {
        let doc = p("one:\n- 2\n- 3\nfour: 5\n");
        assert_eq!(b"2", raw(&doc, &[k("one"), ix(0)]));
        assert_eq!(b"3", raw(&doc, &[k("one"), ix(1)]));
        assert_eq!(b"5", raw(&doc, &[k("four")]));
        let doc = p("a:\n  b:\n  - 1\n  - 2\n  c: hello\n");
        assert_eq!(b"1", raw(&doc, &[k("a"), k("b"), ix(0)]));
        assert_eq!(b"hello", s(&doc, &[k("a"), k("c")]));
    }

    #[test]
    fn indentless_sequence_in_a_nested_mapping_closes_on_dedent_to_an_outer_key() {
        let doc = p("meta:\n  revs:\n  - 1\n  - 2\nother: x\n");
        assert_eq!(b"1", raw(&doc, &[k("meta"), k("revs"), ix(0)]));
        assert_eq!(b"2", raw(&doc, &[k("meta"), k("revs"), ix(1)]));
        assert_eq!(b"x", s(&doc, &[k("other")]));
        let doc = p("a:\n  b:\n    c:\n    - 1\n    - 2\nnext: y\n");
        assert_eq!(b"1", raw(&doc, &[k("a"), k("b"), k("c"), ix(0)]));
        assert_eq!(b"y", s(&doc, &[k("next")]));
    }

    #[test]
    fn mapping_as_an_explicit_key() {
        let doc = p("? earth: blue\n: moon: white\n");
        let kv = first_pair(&doc.ast, doc.ast.root);
        assert_eq!(NodeTag::Mapping, doc.ast.node(kv.key).kind.tag());
        assert_eq!(NodeTag::Mapping, doc.ast.node(kv.value).kind.tag());
        // A tab standing in for the nested mapping's indentation is invalid.
        test_parser_error("?\tkey:\n", "UnexpectedToken");
        test_parser_error("? key:\n:\tkey:\n", "UnexpectedToken");
    }

    #[test]
    fn explicit_key_nested_in_a_deferred_explicit_key() {
        let doc = p("?\n  ? earth\n  : blue\n: x\n");
        let kv = first_pair(&doc.ast, doc.ast.root);
        assert_eq!(NodeTag::Mapping, doc.ast.node(kv.key).kind.tag());
        let inner = first_pair(&doc.ast, kv.key);
        assert_eq!(b"earth", doc.ast.node(inner.key).kind.as_string().unwrap());
        assert_eq!(b"blue", doc.ast.node(inner.value).kind.as_string().unwrap());
        assert_eq!(b"x", doc.ast.node(kv.value).kind.as_string().unwrap());
        // An indentless block key still closes on the `:` at its own column.
        let flat = p("?\n- a\n- b\n: c\n");
        let kv = first_pair(&flat.ast, flat.ast.root);
        assert_eq!(NodeTag::Sequence, flat.ast.node(kv.key).kind.tag());
        assert_eq!(b"c", flat.ast.node(kv.value).kind.as_string().unwrap());
    }

    #[test]
    fn explicit_key_a_property_on_the_question_line_decorates_the_block_key() {
        let doc = p("? &a\n  - a\n  - b\n: *a\n");
        let kv = first_pair(&doc.ast, doc.ast.root);
        let key = doc.ast.node(kv.key);
        assert_eq!(NodeTag::Sequence, key.kind.tag());
        assert_eq!(b"a", doc.ast.anchor_of(key.id).unwrap());
        assert_eq!(key.id, doc.ast.resolve_alias(&doc.ast.node(kv.value)).unwrap());
        // With no key content, the `:` makes an (anchored) null key.
        let empty = p("? &a\n: x\n");
        let kv = first_pair(&empty.ast, empty.ast.root);
        assert_eq!(NodeTag::Null, empty.ast.node(kv.key).kind.tag());
        assert_eq!(b"a", empty.ast.anchor_of(kv.key).unwrap());
    }

    #[test]
    fn anchor_a_property_on_an_implicit_keys_own_line_is_the_keys() {
        for src in ["&a a: b\nc: *a\n", "x:\n  &a a: b\n  c: *a\n", "- &a a: b\n- *a\n"] {
            let doc = p(src);
            let mut seen = false;
            for i in 0..doc.ast.node_count() {
                if doc.ast.anchor_of(i as NodeId).is_none() {
                    continue;
                }
                assert_eq!(b"a", doc.ast.kind_of(i as NodeId).as_string().unwrap());
                seen = true;
            }
            assert!(seen);
        }
        // A tag the same way (`!!str a: b` tags the key).
        let tagged = p("!!str a: b\n");
        let kv = first_pair(&tagged.ast, tagged.ast.root);
        assert!(tagged.ast.tag_of(tagged.ast.root).is_none());
        assert_eq!(b"!!str", tag_text(&tagged.ast, kv.key).unwrap());
        // One on an earlier line is still the mapping's.
        let above = p("k: &m\n  a: b\n");
        assert_eq!(b"m", above.ast.anchor_of(val(&above, &[k("k")]).id).unwrap());
    }

    #[test]
    fn plain_scalar_continuation_may_start_with_an_indicator() {
        assert_eq!(b"a b !c d", s(&p("safe: a b\n      !c d\nnext: 1\n"), &[k("safe")]));
    }

    #[test]
    fn yaml_1_1_resolves_booleans_radix_ints_sexagesimal_and_timestamps() {
        let doc = p11("flag: yes\noff_flag: Off\noctal: 0777\nbinary: 0b1010\nhex: 0x_0A\nunder: 1_000\nbase60: 190:20:30\nfixed: 685_230.15\nwhen: 2001-12-15T02:59:43.1Z\nday: 2002-12-14\n");
        assert_eq!(NodeKind::Boolean(true), val(&doc, &[k("flag")]).kind);
        assert_eq!(NodeKind::Boolean(false), val(&doc, &[k("off_flag")]).kind);
        for key in ["octal", "binary", "hex", "under", "base60"] {
            assert_eq!(NumberKind::Integer, val(&doc, &[k(key)]).kind.as_number().unwrap().kind, "{key}");
        }
        assert_eq!(NumberKind::Float, val(&doc, &[k("fixed")]).kind.as_number().unwrap().kind);
        let NodeKind::Extended(when) = val(&doc, &[k("when")]).kind else { panic!() };
        assert_eq!(ExtKind::OffsetDatetime, when.kind);
        let NodeKind::Extended(day) = val(&doc, &[k("day")]).kind else { panic!() };
        assert_eq!(ExtKind::LocalDate, day.kind);
    }

    #[test]
    fn yaml_1_1_vs_1_2_scalar_divergence() {
        let src = "exp: 1e3\nprefixed_octal: 0o17\nbare_hex: 0x\ninvalid_octal: 08\n";
        let v11 = p11(src);
        // 1.1: all four are plain strings.
        for key in ["exp", "prefixed_octal", "bare_hex", "invalid_octal"] {
            assert_eq!(NodeTag::String, tag(&v11, &[k(key)]));
        }
        // 1.2 core schema: `1e3` is a float and `0o17` an octal int.
        let v12 = p(src);
        assert_eq!(NumberKind::Float, val(&v12, &[k("exp")]).kind.as_number().unwrap().kind);
        assert_eq!(NumberKind::Integer, val(&v12, &[k("prefixed_octal")]).kind.as_number().unwrap().kind);
    }
}
