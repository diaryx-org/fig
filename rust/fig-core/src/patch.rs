//! Port of src/patch.zig.
//!
//! Structure-aware patching: merge one document into another, in place.
//!
//! A patch is an ordinary document. `apply_to` walks it against the target and
//! emits the editor's existing span-splice ops — `set`, `append_to_seq`, the
//! comment ops — one per node it actually has to change. It is a PLANNER over
//! `Editor`, not a second editing engine: every byte it writes is written by
//! the same primitives `fig set` uses, so every guarantee they carry (atomic
//! reparse, ancestor auto-vivification, the block-into-flow refusal) carries
//! here unchanged.
//!
//! What that buys, and what it costs:
//!
//!   * Bytes outside the paths the patch names are **identical**. That is the
//!     whole point, and it is why this walks and splices rather than
//!     parse-merge-reprint.
//!   * Bytes inside them are rendered in the **target's** format and style,
//!     not the patch's. Two documents' formatting cannot both survive into one
//!     file; the target's wins, because the target is the file being kept.
//!     For a scalar that is the whole story. For a CONTAINER the patch
//!     changes, "the target's style" means the target format's PRINTER — the
//!     AST records no flow-vs-block memory, so a YAML `[80, 443]` the patch
//!     rewrites comes back as a block sequence. The value is right and the
//!     spelling is the printer's; a container the patch leaves alone keeps its
//!     bytes, flow spelling included.
//!   * A value the patch and the target already agree on is not spliced at all
//!     (`nodes_equal`, which reads a number by its value, so `31` agrees with
//!     `0x1F`), so re-applying a patch is a no-op on disk rather than a
//!     re-render of every value it mentions. This is load-bearing, not an
//!     optimization: without it a patch that changes one key would rewrite
//!     every other key it names into fig's spelling of it.
//!
//! The merge rule, per node, is:
//!
//!   | patch    | target             | result                                |
//!   |----------|--------------------|---------------------------------------|
//!   | anything | absent             | `set` the whole subtree (one splice)  |
//!   | mapping  | mapping or null    | recurse, entry by entry               |
//!   | sequence | sequence           | `Options.seq` decides                 |
//!   | anything | anything else      | `set` (the patch wins)                |
//!
//! "null" is in the mapping row because a bare `key:` (and an empty document's
//! root) is a container waiting to exist, which the editor's `set` promotes —
//! the same distinction `Editor.blockedByEmptyNode` draws for its own vivify.
//!
//! What this deliberately refuses rather than guesses at:
//!
//!   * Patching THROUGH a YAML alias (`PatchThroughAlias`). A `*name`
//!     target is a reference to a node defined elsewhere; writing "into" it
//!     either edits every other user of that anchor or silently shadows it,
//!     and there is no way to tell from here which the caller meant. The
//!     aliased key case is different and is NOT refused: the editor already
//!     has a policy for a key supplied by a `<<` merge (shadow it with a local
//!     entry), and that policy is a good one.
//!   * A non-mapping patch against the document ROOT
//!     (`PatchRootNotMergeable`) — replacing a whole document is a copy,
//!     not a patch.
//!   * A non-string mapping key (`NonStringPatchKey`). Path navigation
//!     is string-keyed, so there is nowhere to address such an entry.

use crate::diagnostics::{self, Dropped};
use crate::editor::Editor;
use crate::err;
use crate::kit::ast::{Comment, Node, NodeId, NodeKind, Number, NumberKind, PathSegment, SerializeOptions, AST};
use crate::kit::number::{self, LeadingZero, Lexeme};
use crate::registry;
use crate::serialize;
use crate::Result;

type Id = NodeId;

/// What to do when the patch and the target both hold a sequence at the same
/// path. There is no defensible default beyond `replace`: a sequence is
/// sometimes a set (union), sometimes a log (append), and sometimes just a
/// value (replace), and only the caller knows which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeqStrategy {
    /// Take the patch's sequence whole. The default.
    #[default]
    Replace,
    /// Append every item of the patch's sequence to the target's.
    Append,
    /// Append only the items the target does not already hold — compared
    /// structurally (`nodes_equal`), not by source text, so `1` and `0x1` match
    /// across formats while `1` and `1.0` do not.
    Unite,
}

impl SeqStrategy {
    /// `std.meta.stringToEnum`: the member's Zig name.
    pub fn from_name(s: &str) -> Option<SeqStrategy> {
        match s {
            "replace" => Some(SeqStrategy::Replace),
            "append" => Some(SeqStrategy::Append),
            "unite" => Some(SeqStrategy::Unite),
            _ => None,
        }
    }
}

/// Whose comments win where both documents carry one on the same entry. Note
/// that comments NESTED inside a subtree the patch contributes whole always
/// ride along with it (they are part of what was rendered); this decides only
/// the entry-level conflict, plus whether patch comments are carried at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommentStrategy {
    /// Keep the target's comment; contribute the patch's only where the target
    /// has none. The default — same principle as the rest of this module,
    /// which is that the target is the document being kept.
    #[default]
    Ours,
    /// The patch's comment replaces the target's wherever the patch has one.
    Theirs,
    /// Carry no comment from the patch at all, at any depth. The target's own
    /// comments are still untouched.
    None,
}

impl CommentStrategy {
    /// `std.meta.stringToEnum`: the member's Zig name.
    pub fn from_name(s: &str) -> Option<CommentStrategy> {
        match s {
            "ours" => Some(CommentStrategy::Ours),
            "theirs" => Some(CommentStrategy::Theirs),
            "none" => Some(CommentStrategy::None),
            _ => None,
        }
    }

    /// `@tagName`.
    pub fn name(self) -> &'static str {
        match self {
            CommentStrategy::Ours => "ours",
            CommentStrategy::Theirs => "theirs",
            CommentStrategy::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub seq: SeqStrategy,
    pub comments: CommentStrategy,
    /// Style knobs for rendering a patch subtree into the target's syntax —
    /// the same options `fig get`/`fmt` expose, honored by whichever formats
    /// honor them there. `strip_comments` is forced on under
    /// `CommentStrategy::None`.
    pub serialize: SerializeOptions,
}

/// What `apply_to` did, for the caller to report. `changed` is the question a
/// CLI actually asks; the individual counts are for a summary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Values that existed and were rewritten.
    pub replaced: usize,
    /// Keys (or whole subtrees) that did not exist and were created.
    pub added: usize,
    /// Sequence items appended under `SeqStrategy::Append`/`Unite`.
    pub appended: usize,
    /// Paths removed by `apply_to`'s `deletes`.
    pub deleted: usize,
    /// Values the patch names that the target already agreed on, so nothing
    /// was spliced.
    pub unchanged: usize,
    /// Comments the patch carried that the TARGET FORMAT cannot hold (strict
    /// JSON), or that its syntax cannot hold in that position (a multi-line
    /// trailing comment). Dropped rather than made a hard error: losing trivia
    /// should not fail an otherwise-good patch, but it should be reportable.
    pub comments_dropped: usize,
}

impl Stats {
    pub fn changed(&self) -> bool {
        self.replaced + self.added + self.appended + self.deleted > 0
    }
}

/// One path to remove from the target after the merge. The final segment
/// decides which removal it is, exactly as `fig delete` does: a key deletes
/// that mapping entry (with the comments it owns), an index removes that item
/// from the parent sequence.
pub type Deletion<'a> = &'a [PathSegment<'a>];

/// What patch subtrees are rendered as: a registered language's entry,
/// whose printer is reached through its vtable.
pub type Target = serialize::Target;

/// Merge `patch`'s subtree at `from` into the document `editor` holds, landing
/// it at `at` (the document root when empty), then apply `deletes`.
///
/// `target` is what patch subtrees are RENDERED as before being spliced. It
/// must be the format `editor` is editing — a mismatch produces text that
/// either fails to reparse (caught by the editor, which rolls back) or, worse,
/// reparses as something else.
///
/// `patch` must not carry unresolved YAML aliases: materialize it first. An
/// alias in a patch refers to an anchor in the PATCH file, which the target
/// has never seen, so splicing one verbatim would produce a dangling
/// reference.
#[allow(clippy::too_many_arguments)]
pub fn apply_to(
    editor: &mut Editor<'_>,
    target: &Target,
    at: &[PathSegment<'_>],
    patch: &AST,
    from: Id,
    deletes: &[&[PathSegment<'_>]],
    options: Options,
) -> Result<Stats> {
    let mut w = Walker { editor, target, options, path: Vec::new(), stats: Stats::default() };
    for seg in at {
        w.path.push(own(*seg));
    }
    w.merge_value(patch, from)?;

    // Deletions run last, so a patch can both write a key and remove a
    // different one without their order mattering to the caller.
    for path in deletes {
        w.delete(path)?;
    }
    Ok(w.stats)
}

/// A path segment the walker holds: its key copied, since the walker's path
/// outlives no AST it names but crosses every splice.
#[derive(Debug, Clone)]
enum Seg {
    Key(Vec<u8>),
    Index(usize),
}

fn own(seg: PathSegment<'_>) -> Seg {
    match seg {
        PathSegment::Key(k) => Seg::Key(k.to_vec()),
        PathSegment::Index(i) => Seg::Index(i),
    }
}

fn borrow(path: &[Seg]) -> Vec<PathSegment<'_>> {
    path.iter()
        .map(|s| match s {
            Seg::Key(k) => PathSegment::Key(k),
            Seg::Index(i) => PathSegment::Index(*i),
        })
        .collect()
}

struct Walker<'e, 'l, 't> {
    editor: &'e mut Editor<'l>,
    target: &'t Target,
    options: Options,
    /// The target path currently being merged into. Grows and shrinks as
    /// `merge_mapping` descends.
    path: Vec<Seg>,
    stats: Stats,
}

/// A target node, as far as the walker needs it once the parse it came from
/// is gone: its id and whether it is an alias, a mapping, a null or a
/// sequence.
#[derive(Debug, Clone, Copy)]
struct TargetNode {
    id: Id,
    tag: crate::kit::ast::NodeTag,
}

impl Walker<'_, '_, '_> {
    /// The target's value at the current path, or `None` when the path does
    /// not resolve. Never held across a splice: the `Document` it comes from
    /// is freed and rebuilt by every `replace_at_span`.
    fn target_value(&self) -> Result<Option<TargetNode>> {
        let parsed = self.editor.get_parsed()?;
        let path = borrow(&self.path);
        Ok(parsed.ast.get_val_by_path(&path).ok().map(|n: Node<'_>| TargetNode { id: n.id, tag: n.kind.tag() }))
    }

    fn merge_value(&mut self, patch: &AST, id: Id) -> Result<()> {
        use crate::kit::ast::NodeTag;
        let kind = patch.node(id).kind;
        let target = self.target_value()?;

        // A `*name` in the TARGET is a reference to a node defined
        // elsewhere in it; see this module's header for why that is a
        // refusal rather than a policy.
        if let Some(t) = target {
            if t.tag == NodeTag::Alias {
                return Err(err!(PatchThroughAlias));
            }
        }

        match kind {
            NodeKind::Mapping(_) => {
                if let Some(t) = target {
                    // A null target is a container waiting to exist, which
                    // `set` promotes on the way down — so descend into it
                    // rather than clobbering it with a rendered `{}`.
                    if t.tag == NodeTag::Mapping || t.tag == NodeTag::Null {
                        return self.merge_mapping(patch, id);
                    }
                }
            }
            NodeKind::Sequence(_) => {
                if self.options.seq != SeqStrategy::Replace {
                    if let Some(t) = target {
                        if t.tag == NodeTag::Sequence {
                            return self.merge_sequence(patch, id);
                        }
                    }
                }
            }
            _ => {}
        }
        self.set_value(patch, id, target)
    }

    fn merge_mapping(&mut self, patch: &AST, id: Id) -> Result<()> {
        let NodeKind::Mapping(mut next) = patch.node(id).kind else { return Err(err!(InvalidPatchDocument)) };
        while let Some(entry_id) = next {
            let entry = patch.node(entry_id);
            next = entry.next_sibling;
            let kv = match entry.kind {
                NodeKind::KeyValue(kv) => kv,
                // A mapping whose children are not entries is not a
                // document any reader in this library produces.
                _ => return Err(err!(InvalidPatchDocument)),
            };
            let key = match patch.node(kv.key).kind {
                NodeKind::String(s) => s,
                _ => return Err(err!(NonStringPatchKey)),
            };

            self.path.push(Seg::Key(key.to_vec()));
            let r = self.merge_value(patch, kv.value).and_then(|_| self.apply_comments(patch, entry_id));
            self.path.pop();
            r?;
        }
        Ok(())
    }

    fn merge_sequence(&mut self, patch: &AST, id: Id) -> Result<()> {
        let NodeKind::Sequence(mut next) = patch.node(id).kind else { return Ok(()) };
        while let Some(item_id) = next {
            next = patch.node(item_id).next_sibling;
            if self.options.seq == SeqStrategy::Unite && self.target_has_item(patch, item_id)? {
                self.stats.unchanged += 1;
                continue;
            }
            let text = self.render(patch, item_id, self.path.len() + 1)?;
            let path = borrow(&self.path);
            self.editor.append_to_seq(&path, &text)?;
            self.stats.appended += 1;
        }
        Ok(())
    }

    /// Whether the target's sequence at the current path already holds an
    /// item structurally equal to `item_id`. Re-read on every call rather
    /// than once: each append reparses, and comparing against the sequence
    /// as it now stands is what makes `unite` dedupe WITHIN the patch too.
    fn target_has_item(&self, patch: &AST, item_id: Id) -> Result<bool> {
        let parsed = self.editor.get_parsed()?;
        let path = borrow(&self.path);
        let Ok(seq) = parsed.ast.get_val_by_path(&path) else { return Ok(false) };
        let NodeKind::Sequence(mut next) = seq.kind else { return Ok(false) };
        while let Some(id) = next {
            if nodes_equal_with(patch, item_id, &parsed.ast, id, Zeros::of(self.target)) {
                return Ok(true);
            }
            next = parsed.ast.node(id).next_sibling;
        }
        Ok(false)
    }

    fn set_value(&mut self, patch: &AST, id: Id, target: Option<TargetNode>) -> Result<()> {
        // Already agreed on: splice nothing. See the header — this is what
        // keeps a patch's diff the size of what it actually changes.
        if let Some(t) = target {
            let parsed = self.editor.get_parsed()?;
            if nodes_equal_with(patch, id, &parsed.ast, t.id, Zeros::of(self.target)) {
                self.stats.unchanged += 1;
                return Ok(());
            }
        }
        if self.path.is_empty() {
            return Err(err!(PatchRootNotMergeable));
        }

        let text = self.render(patch, id, self.path.len())?;

        // `set` upserts and vivifies missing ancestors, but it addresses a
        // mapping key. An indexed tail names an existing item instead —
        // there is nothing to create — so it goes through the plain value
        // replacement. A string goes through their string forms, so a
        // string it replaces keeps its quote style: the patch's own style
        // is the printer's by now, and the target's is the one kept.
        let last_is_key = matches!(self.path.last(), Some(Seg::Key(_)));
        let path = borrow(&self.path);
        match patch.node(id).kind {
            NodeKind::String(string) => {
                if last_is_key {
                    self.editor.set_string(&path, &text, string)?
                } else {
                    self.editor.replace_string_at_path(&path, &text, string)?
                }
            }
            _ => {
                if last_is_key {
                    self.editor.set(&path, &text)?
                } else {
                    self.editor.replace_val_at_path(&path, &text)?
                }
            }
        }
        if target.is_none() {
            self.stats.added += 1;
        } else {
            self.stats.replaced += 1;
        }
        Ok(())
    }

    /// Render the patch subtree at `id` as a value fragment in the target's
    /// format, to land `depth` levels below the document root.
    ///
    /// A value the target cannot hold THERE is refused before it is
    /// rendered, with the error its printer raises for one it cannot hold
    /// anywhere. The printer cannot tell: a mapping is a fine dotenv
    /// document and prints as one, `x=1`, which spliced after `n=` reads
    /// back as the string `x=1`.
    fn render(&self, patch: &AST, id: Id, depth: usize) -> Result<Vec<u8>> {
        if let Some(dropped) = diagnostics::first_dropped(patch, id, self.target, depth) {
            return Err(match dropped {
                Dropped::Null => err!(NullUnsupported),
                Dropped::Container => err!(UnsupportedValue),
            });
        }

        let mut options = self.options.serialize;
        if self.options.comments == CommentStrategy::None {
            options.strip_comments = true;
        }
        // A value spliced after `key = ` lands inline, so a container is
        // asked for in flow: a format whose block spelling only parses as
        // lines of its own (figl's `* ` items and section headers) would
        // re-read one there as a bare string. A format with one spelling
        // ignores it.
        options.flow = true;
        // What the editor takes, not a document: plist's bare element
        // rather than a wrapped `<plist>`, a NestedText scalar's plain
        // text rather than a `>` block.
        options.splice = true;

        let mut w = Vec::new();
        match self.target {
            Target::Canonical => serialize::serialize_node_with(patch, &mut w, &Target::Canonical, id, &options)?,
            Target::Language(e) => registry::print_node_with(e, patch, id, &options, &mut w)?,
        }
        // Every printer terminates a DOCUMENT with a newline. What the
        // editor takes is a VALUE — the same text a `fig set` argument
        // supplies, which never carries one — and it splices what it is
        // given: leave the newline on and an appended flow item lands
        // before the `]` on a line of its own.
        while w.last() == Some(&b'\n') {
            w.pop();
        }
        Ok(w)
    }

    /// Carry the patch entry's own leading/trailing comments onto the node
    /// just merged, as `Options.comments` directs. Trivia: a format that
    /// cannot hold the comment counts it dropped rather than failing the
    /// patch.
    fn apply_comments(&mut self, patch: &AST, entry_id: Id) -> Result<()> {
        if self.options.comments == CommentStrategy::None {
            return Ok(());
        }
        let leading_comments = patch.comments(patch.leading_comment_anchor(entry_id));
        let trailing_comments = patch.comments(patch.trailing_comment_anchor(entry_id));
        let leading: &[Comment<'_>] = &leading_comments.leading;
        let trailing = trailing_comments.trailing;
        if leading.is_empty() && trailing.is_none() {
            return Ok(());
        }

        if !leading.is_empty() {
            self.carry_leading(leading)?;
        }
        if let Some(c) = trailing {
            self.carry_trailing(c)?;
        }
        Ok(())
    }

    fn carry_leading(&mut self, leading: &[Comment<'_>]) -> Result<()> {
        let path = borrow(&self.path);
        let existing = match self.editor.get_leading_comment(&path) {
            Ok(e) => e,
            // No comment syntax in this format, or the entry isn't there
            // to comment on (an empty mapping the merge contributed
            // nothing for).
            Err(e) if e.is("CommentsUnsupported") => {
                self.drop_comment();
                return Ok(());
            }
            Err(e) if e.is("NotFound") => return Ok(()),
            Err(e) => return Err(e),
        };
        if self.options.comments == CommentStrategy::Ours && existing.is_some() {
            return Ok(());
        }

        let mut text: Vec<u8> = Vec::new();
        for (i, c) in leading.iter().enumerate() {
            if i > 0 {
                text.push(b'\n');
            }
            text.extend_from_slice(c.text);
        }
        if existing.is_some() {
            self.editor.delete_leading_comments(&path)?;
        }
        match self.editor.add_leading_comment(&path, &text) {
            Ok(()) => Ok(()),
            // The entry landed inside a one-line flow collection, where a
            // comment has no line of its own to sit on (and would be
            // discarded on the next parse anyway). Trivia: count it
            // dropped, like a format with no comment syntax at all.
            Err(e) if e.is("CommentsUnanchored") => {
                self.drop_comment();
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn carry_trailing(&mut self, comment: Comment<'_>) -> Result<()> {
        // A same-line comment is one line by definition; a block comment
        // that spans several has no trailing spelling to downgrade to.
        if comment.text.contains(&b'\n') {
            self.drop_comment();
            return Ok(());
        }
        let path = borrow(&self.path);
        let existing = match self.editor.get_trailing_comment(&path) {
            Ok(e) => e,
            Err(e) if e.is("CommentsUnsupported") => {
                self.drop_comment();
                return Ok(());
            }
            Err(e) if e.is("NotFound") => return Ok(()),
            Err(e) => return Err(e),
        };
        if self.options.comments == CommentStrategy::Ours && existing.is_some() {
            return Ok(());
        }
        match self.editor.set_trailing_comment(&path, comment.text) {
            Ok(()) => Ok(()),
            Err(e)
                if e.is("CommentsUnsupported")
                    || e.is("MultilineComment")
                    || e.is("CommentsUnanchored")
                    || e.is("CommentInsideValue") =>
            {
                self.drop_comment();
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn drop_comment(&mut self) {
        self.stats.comments_dropped += 1;
    }

    /// Remove one path from the target, keyed on its final segment exactly
    /// as `fig delete` is. A path that isn't there is not an error: a
    /// patch that removes a key twice, or removes one an earlier run
    /// already removed, has still arrived at what it asked for.
    fn delete(&mut self, path: &[PathSegment<'_>]) -> Result<()> {
        let Some(last) = path.last() else { return Err(err!(PatchRootNotMergeable)) };
        let result = match last {
            PathSegment::Key(_) => self.editor.delete_key(path),
            PathSegment::Index(i) => self.editor.remove_seq_item(&path[..path.len() - 1], *i),
        };
        match result {
            Ok(()) => {}
            Err(e) if e.is("NotFound") => return Ok(()),
            Err(e) => return Err(e),
        }
        self.stats.deleted += 1;
        Ok(())
    }
}

/// Structural equality of two subtrees, possibly from different documents in
/// different formats. Comments, tags and anchors are excluded — they are
/// trivia and side-tables, not the value — so this answers exactly the
/// question the merge asks: "would splicing this change what the document
/// means?"
///
/// A number compares by its value, not its lexeme (`number::eql_value`): a
/// target's `0x1F` agrees with a patch's `31`, and `1.50` with `1.5`, which is
/// what lets a patch that came through JSON (`fig convert -o json | jq | fig
/// patch`) leave every number it did not change in the target's spelling. An
/// integer never agrees with a float, `1` with `1.0` included, and a JSON5
/// `Infinity` is the float it spells. Strings, booleans and nulls already
/// reach here as values, so `'debug'` is `debug` and `~` is `null`: what the
/// target's dialect read them as, not how it spelled them.
///
/// A leading-zero integer (`0755`) means what only its dialect can say, so
/// here it agrees only with the same bytes; `nodes_equal_with` is told.
pub fn nodes_equal(a: &AST, a_id: Id, b: &AST, b_id: Id) -> bool {
    nodes_equal_with(a, a_id, b, b_id, Zeros::default())
}

/// `nodes_equal`, told what a leading-zero integer means in each document.
/// `Zeros::of` says it for a target.
pub fn nodes_equal_with(a: &AST, a_id: Id, b: &AST, b_id: Id, zeros: Zeros) -> bool {
    let x = a.node(a_id).kind;
    let y = b.node(b_id).kind;
    if let (Some(p), Some(q)) = (number_of(x), number_of(y)) {
        return number::eql_value(
            Lexeme { raw: p.raw, float: p.kind == NumberKind::Float, leading_zero: zeros.a },
            Lexeme { raw: q.raw, float: q.kind == NumberKind::Float, leading_zero: zeros.b },
        );
    }
    if x.tag() != y.tag() {
        return false;
    }
    match (x, y) {
        (NodeKind::Null, _) => true,
        (NodeKind::Boolean(v), NodeKind::Boolean(w)) => v == w,
        (NodeKind::String(v), NodeKind::String(w)) => v == w,
        (NodeKind::Extended(v), NodeKind::Extended(w)) => v.eql(&w),
        (NodeKind::Alias(v), NodeKind::Alias(w)) => v == w,
        (NodeKind::KeyValue(kv), NodeKind::KeyValue(lw)) => {
            nodes_equal_with(a, kv.key, b, lw.key, zeros) && nodes_equal_with(a, kv.value, b, lw.value, zeros)
        }
        (NodeKind::Sequence(first), NodeKind::Sequence(other)) => children_equal(a, first, b, other, zeros),
        (NodeKind::Mapping(first), NodeKind::Mapping(other)) => children_equal(a, first, b, other, zeros),
        // A number is compared by value above.
        _ => false,
    }
}

/// What a leading-zero integer means in `nodes_equal_with`'s `a` and in its `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Zeros {
    pub a: LeadingZero,
    pub b: LeadingZero,
}

impl Zeros {
    /// A patch, whose dialect `apply_to` is not told, against `target`. YAML
    /// 1.1 reads `0755` as octal; every other format fig edits either has no
    /// such integer or reads it as decimal, as `number::write_canonical` does.
    pub fn of(target: &Target) -> Zeros {
        let yaml_1_1 = match target {
            Target::Language(e) => e.name == "yaml-1.1",
            Target::Canonical => false,
        };
        Zeros { a: LeadingZero::Unknown, b: if yaml_1_1 { LeadingZero::Octal } else { LeadingZero::Decimal } }
    }
}

/// The number `kind` holds: a number, or a JSON5 non-finite one, which rides
/// in an extended scalar because no JSON number can hold it.
fn number_of(kind: NodeKind<'_>) -> Option<Number<'_>> {
    match kind {
        NodeKind::Number(n) => Some(n),
        NodeKind::Extended(e) if e.kind == crate::kit::ast::ExtKind::NumberSpecial => {
            Some(Number { raw: e.text, kind: NumberKind::Float })
        }
        _ => None,
    }
}

/// Walk two containers' sibling chains in step. Order-sensitive, for both
/// kinds: a reordered sequence is a different value, and a reordered mapping
/// is a different set of BYTES — and since the merge recurses into a mapping
/// it finds on both sides, this only ever compares mappings it is about to
/// splice whole anyway.
fn children_equal(a: &AST, a_first: Option<Id>, b: &AST, b_first: Option<Id>, zeros: Zeros) -> bool {
    let mut p = a_first;
    let mut q = b_first;
    while let (Some(pp), Some(qq)) = (p, q) {
        if !nodes_equal_with(a, pp, b, qq, zeros) {
            return false;
        }
        p = a.node(pp).next_sibling;
        q = b.node(qq).next_sibling;
    }
    p.is_none() && q.is_none()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::global;

    /// Parse `source` as `target`, merge `patch_source` (in `patch`) into it,
    /// and return the edited bytes; `None` while either dialect is not
    /// registered.
    fn patch_for_test(target: &str, patch: &str, source: &[u8], patch_source: &[u8], options: Options) -> Option<Vec<u8>> {
        let te = global().entry_by_name(target)?;
        let pe = global().entry_by_name(patch)?;
        let patch_doc = pe.parse(patch_source).unwrap();
        let mut editor = Editor::new(te.lang(), te.dialect);
        editor.init(source).unwrap();
        let t = Target::Language(te.clone());
        apply_to(&mut editor, &t, &[], &patch_doc.ast, patch_doc.ast.root, &[], options).unwrap();
        Some(editor.source().to_vec())
    }

    #[test]
    fn a_scalar_patch_touches_only_the_keys_it_names() {
        let Some(out) = patch_for_test(
            "toml",
            "json",
            b"name = \"api\" # must match the DNS record\nreplicas = 2\nports = [80, 443]\n",
            b"{\"replicas\": 5}\n",
            Options::default(),
        ) else {
            return;
        };
        assert_eq!(
            String::from_utf8_lossy(&out),
            "name = \"api\" # must match the DNS record\nreplicas = 5\nports = [80, 443]\n"
        );
    }

    #[test]
    fn a_value_both_sides_already_agree_on_is_not_spliced() {
        let (Some(te), Some(pe)) = (global().entry_by_name("toml"), global().entry_by_name("json")) else { return };
        let patch_doc = pe.parse(b"{\"name\": \"api\", \"n\": 31}\n").unwrap();
        let mut editor = Editor::new(te.lang(), te.dialect);
        editor.init(b"name = 'api'\nn = 0x1F\n").unwrap();
        let stats = apply_to(&mut editor, &Target::Language(te.clone()), &[], &patch_doc.ast, patch_doc.ast.root, &[], Options::default())
            .unwrap();
        assert_eq!(editor.source(), b"name = 'api'\nn = 0x1F\n");
        assert_eq!(stats.unchanged, 2);
        assert!(!stats.changed());
    }

    #[test]
    fn a_non_mapping_patch_against_the_root_is_refused() {
        let (Some(te), Some(pe)) = (global().entry_by_name("json"), global().entry_by_name("json")) else { return };
        let patch_doc = pe.parse(b"[1]\n").unwrap();
        let mut editor = Editor::new(te.lang(), te.dialect);
        editor.init(b"{\"a\": 1}\n").unwrap();
        let r = apply_to(&mut editor, &Target::Language(te.clone()), &[], &patch_doc.ast, patch_doc.ast.root, &[], Options::default());
        assert_eq!(r.unwrap_err(), err!(PatchRootNotMergeable));
    }

    #[test]
    fn nodes_equal_reads_numbers_by_value() {
        let Some(e) = global().entry_by_name("json5") else { return };
        let a = e.parse(b"[31, 1.5, 1]").unwrap();
        let b = e.parse(b"[0x1F, 1.50, 1.0]").unwrap();
        let (NodeKind::Sequence(Some(a0)), NodeKind::Sequence(Some(b0))) = (a.ast.node(a.ast.root).kind, b.ast.node(b.ast.root).kind)
        else {
            panic!()
        };
        assert!(nodes_equal(&a.ast, a0, &b.ast, b0));
        let a1 = a.ast.node(a0).next_sibling.unwrap();
        let b1 = b.ast.node(b0).next_sibling.unwrap();
        assert!(nodes_equal(&a.ast, a1, &b.ast, b1));
        let a2 = a.ast.node(a1).next_sibling.unwrap();
        let b2 = b.ast.node(b1).next_sibling.unwrap();
        assert!(!nodes_equal(&a.ast, a2, &b.ast, b2));
    }
}
