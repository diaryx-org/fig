//! Port of src/cli/edit_ops.zig.
//!
//! In-place editing plumbing shared by `replace`/`rename`/`set`/`insert`/`delete`/
//! `comment`: the single span-splice path (`apply_edit`) behind every
//! structural op, and the small helpers (value rendering, empty-document
//! seeds) it leans on.

use std::cell::RefCell;

use fig_core::editor::Editor;
use fig_core::kit::ast::{NodeKind, AST};
use fig_core::registry::Entry;
use fig_core::{err, Result};

use crate::fileio::{self, Input};
use crate::types::{self, segs, EditOp, Format, PathSeg};
use crate::value_arg::{self, Layout, Value};

/// Apply one in-place edit to `content` (a complete document parsed under
/// `e`'s dialect) and return the new bytes. The single span-splice path behind
/// both the `replace`/`rename` and `comment` actions.
pub fn apply_edit(e: &Entry, content: &[u8], path: &[PathSeg], text: &[u8], op: &EditOp) -> Result<Vec<u8>> {
    let mut editor = Editor::new(e.lang(), e.dialect);
    editor.init(content)?;
    // `[-]` anywhere in the path names the last item that is there. The op
    // receives the parent of the slot it creates, so an append's own `[-]`
    // never reaches here.
    let resolved = {
        let parsed = editor.get_parsed()?;
        resolve_last(&parsed.ast, path)
    };
    if let Err(error) = apply_op(&mut editor, &resolved, text, op) {
        // `init` parsed `content`, so a parse failure from here on is about
        // the text the caller spliced in, not the file — and it arrives as a
        // plain `InvalidNumber`/`UnexpectedToken`/… that reads as if the FILE
        // were malformed. Re-label it so the CLI can blame (and quote) the
        // argument instead; see `diag_report::report_bad_edit_text`.
        if editor.splice_rejected {
            return Err(err!(InvalidEditText));
        }
        return Err(error);
    }
    Ok(editor.source().to_vec())
}

/// The op dispatch behind `apply_edit`, split out only so its errors can be
/// caught in one place there.
fn apply_op(editor: &mut Editor<'_>, path: &[PathSeg], text: &[u8], op: &EditOp) -> Result<()> {
    let p = segs(path);
    match op {
        EditOp::ReplaceValue => editor.replace_val_at_path(&p, text),
        // The new key is a NAME too, spelled the same way. A name the
        // mapping already holds is reported as `rename`'s refusal, not as
        // `insert`'s, whose help would send the user to `set`.
        EditOp::ReplaceKey => editor.replace_named_key(&p, text).map_err(|e| if e.is("DuplicateKey") { err!(RenameTargetExists) } else { e }),
        EditOp::AddLeadingComment => editor.add_leading_comment(&p, text),
        EditOp::SetTrailingComment => editor.set_trailing_comment(&p, text),
        EditOp::DeleteLeadingComments => editor.delete_leading_comments(&p),
        EditOp::DeleteTrailingComment => editor.delete_trailing_comment(&p),
        // The CLI's key is a NAME; `insert_key` takes key syntax, so it is
        // spelled as the format spells one first — `.name` in ZON, quoted
        // for strict JSON — exactly as `set`'s insert branch spells it.
        EditOp::InsertKey(key) => editor.insert_named_key(&p, key, text),
        EditOp::Set => editor.set(&p, text),
        EditOp::ReplaceString(string) => editor.replace_string_at_path(&p, text, string),
        EditOp::SetString(string) => editor.set_string(&p, text, string),
        EditOp::SetSequence(items) => {
            let items: Vec<&[u8]> = items.iter().map(|i| i.as_slice()).collect();
            editor.set_sequence(&p, &items)
        }
        EditOp::AppendSeq => editor.append_to_seq(&p, text),
        EditOp::SetAppend => editor.set_append(&p, text),
        EditOp::PrependSeq => editor.prepend_to_seq(&p, text),
        EditOp::InsertSeqItem(index) => editor.insert_item(&p, *index, text),
        EditOp::DeleteKey => {
            if !delete_if_section(editor, path)? {
                editor.delete_key(&p)?;
            }
            Ok(())
        }
        EditOp::RemoveSeqItem(index) => {
            // A sequence of sections — TOML's `[[x]]` — has its elements
            // addressed by index, so the section check wants the full path.
            let mut full: Vec<PathSeg> = path.to_vec();
            full.push(PathSeg::Index(last_index(editor, path, *index)));
            if !delete_if_section(editor, &full)? {
                editor.remove_seq_item(&p, *index)?;
            }
            Ok(())
        }
    }
}

/// `delete` on a path whose value is a SECTION — a TOML table or
/// array-of-tables element, an INI section, a fig block container, a runtime
/// language's section — deletes the whole container. `delete_key` and
/// `remove_seq_item` refuse one, because its body is lines they cannot see;
/// `delete_container` is the op that gathers every region of it. Returns
/// whether it did, so the caller runs the ordinary op otherwise.
fn delete_if_section(editor: &mut Editor<'_>, path: &[PathSeg]) -> Result<bool> {
    if !editor.is_section_format() {
        return Ok(false);
    }
    let p = segs(path);
    let section = {
        let parsed = editor.get_parsed()?;
        match parsed.ast.get_val_by_path(&p) {
            Ok(node) => parsed.is_section(&node),
            Err(_) => return Ok(false),
        }
    };
    if !section {
        return Ok(false);
    }
    editor.delete_container(&p)?;
    Ok(true)
}

/// `index`, with the end sentinel `[-]`/`[$]` (`usize::MAX`) resolved to the
/// last item of the sequence at `path`. Anything that is not a non-empty
/// sequence is returned as given, for `remove_seq_item` to refuse.
fn last_index(editor: &Editor<'_>, path: &[PathSeg], index: usize) -> usize {
    if index != types::APPEND_INDEX {
        return index;
    }
    let Ok(parsed) = editor.get_parsed() else { return index };
    last_index_in(&parsed.ast, path).unwrap_or(index)
}

/// The index of the last item of the sequence at `path` in `ast`, or `None`
/// when there is no non-empty sequence there.
fn last_index_in(ast: &AST, path: &[PathSeg]) -> Option<usize> {
    let seq = ast.get_val_by_path(&segs(path)).ok()?;
    let NodeKind::Sequence(_) = seq.kind else { return None };
    let mut item = ast.child(&seq).ok()??;
    let mut last = 0usize;
    while let Some(nxt) = ast.next(&item) {
        item = nxt;
        last += 1;
    }
    Some(last)
}

/// `path` with every `[-]`/`[$]` resolved, left to right, to the last item
/// of the sequence it indexes — so `list[-].name` reads, replaces or
/// comments the last item's `name`. A sentinel with no non-empty sequence
/// under it stays as it is, and the lookup that follows reports the path
/// missing.
pub fn resolve_last(ast: &AST, path: &[PathSeg]) -> Vec<PathSeg> {
    let mut out = path.to_vec();
    for i in 0..out.len() {
        if out[i] != PathSeg::Index(types::APPEND_INDEX) {
            continue;
        }
        if let Some(last) = last_index_in(ast, &out[..i]) {
            out[i] = PathSeg::Index(last);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn apply_to_file(
    e: &Entry,
    file: &mut Input,
    path: &[PathSeg],
    text: &[u8],
    value: &OpValue,
    op: &EditOp,
    format: Format,
) -> Result<()> {
    let content = fileio::read_all(file)?;
    let edited = apply_value_edit(e, &content, path, text, value, op, format)?;
    file.rewrite(&edited)
}

thread_local! {
    /// The text a value argument was written as when the document refused it —
    /// set just before `apply_value_edit` returns `InvalidEditText` for a
    /// rendered value, so the report can show the spelling that was refused
    /// rather than only what was typed. `None` otherwise. The CLI makes one
    /// edit per run, so one slot is all there is to hold.
    pub static REFUSED_RENDERING: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// The current `REFUSED_RENDERING`.
pub fn refused_rendering() -> Option<Vec<u8>> {
    REFUSED_RENDERING.with(|r| r.borrow().clone())
}

/// `apply_edit` with `value` rendered into `format` first — the one place a
/// value argument meets the document (`apply_to_file`). Two retries live here,
/// each on the one copy of `content`, so neither reads or parses the file again:
///
///   * a value rendered in the format's own layout (a YAML block) whose site
///     is inside a flow collection (`k: {a: 1}`) is refused having written
///     nothing, and goes again on one line (`Layout::Flow`), which is what
///     fits there;
///   * an op that may create a key (`InsertKey`, `Set`) whose splice is
///     refused goes again with `0` — a value every format spells — and when
///     that is refused too the KEY is what the document would not take:
///     `InvalidEditKey`, so the report names the key, not the value.
#[allow(clippy::too_many_arguments)]
pub fn apply_value_edit(
    e: &Entry,
    content: &[u8],
    path: &[PathSeg],
    text: &[u8],
    value: &OpValue,
    op: &EditOp,
    format: Format,
) -> Result<Vec<u8>> {
    REFUSED_RENDERING.with(|r| *r.borrow_mut() = None);
    // How deep the value lands: at `path` for a replacement or an upsert,
    // one level below it for a new key or a sequence item.
    let depth = path.len()
        + match op {
            EditOp::InsertKey(_)
            | EditOp::AppendSeq
            | EditOp::SetAppend
            | EditOp::PrependSeq
            | EditOp::InsertSeqItem(_)
            | EditOp::SetSequence(_) => 1,
            _ => 0,
        };
    if let Some(v) = &value.one {
        value_arg::refuse_dropped(v, format, depth)?;
    }
    for v in &value.items {
        value_arg::refuse_dropped(v, format, depth)?;
    }
    let mut r = render_edit(text, value, op, format, Layout::Natural)?;
    match apply_edit(e, content, path, &r.text, &r.op) {
        Ok(edited) => return Ok(edited),
        Err(error) => match error.name() {
            "BlockValueIntoFlow" => {
                if value.any() {
                    r = render_edit(text, value, op, format, Layout::Flow)?;
                    match apply_edit(e, content, path, &r.text, &r.op) {
                        Ok(edited) => return Ok(edited),
                        Err(e2) => {
                            if !e2.is("InvalidEditText") {
                                return Err(e2);
                            }
                        }
                    }
                } else {
                    return Err(error);
                }
            }
            "InvalidEditText" => {}
            _ => return Err(error),
        },
    }
    // The splice was refused (`InvalidEditText`): say which half of it.
    if let EditOp::InsertKey(_) | EditOp::Set = op {
        if let Err(e3) = apply_edit(e, content, path, b"0", op) {
            if e3.is("InvalidEditText") {
                return Err(err!(InvalidEditKey));
            }
        }
    }
    if value.one.is_some() {
        REFUSED_RENDERING.with(|x| *x.borrow_mut() = Some(r.text.clone()));
    }
    Err(err!(InvalidEditText))
}

/// Read back a comment from `content` (parsed under `e`'s dialect) without
/// writing: the trailing (same-line) comment on the value at `path` when
/// `inline_comment`, else the own-line block above the node. Returns `None`
/// when there is no such comment. The read-only twin of `apply_edit`'s
/// comment ops.
pub fn get_comment(e: &Entry, content: &[u8], path: &[PathSeg], inline_comment: bool) -> Result<Option<Vec<u8>>> {
    let mut editor = Editor::new(e.lang(), e.dialect);
    editor.init(content)?;
    let resolved = {
        let parsed = editor.get_parsed()?;
        resolve_last(&parsed.ast, path)
    };
    let p = segs(&resolved);
    if inline_comment {
        editor.get_trailing_comment(&p)
    } else {
        editor.get_leading_comment(&p)
    }
}

pub fn get_comment_from_file(e: &Entry, file: &mut Input, path: &[PathSeg], inline_comment: bool) -> Result<Option<Vec<u8>>> {
    let content = fileio::read_all(file)?;
    get_comment(e, &content, path, inline_comment)
}

/// The value an op carries, read (`value_arg::read`) but not yet rendered:
/// `render_edit` spells it once the target format is known. `one` is the
/// value of `set`/`insert`/`replace`; `items` are `set --seq`'s. Empty for an op
/// with no value (a comment, a delete, a key rename), whose text is spliced
/// as it stands.
#[derive(Debug, Clone, Default)]
pub struct OpValue {
    pub one: Option<Value>,
    pub items: Vec<Value>,
}

impl OpValue {
    fn any(&self) -> bool {
        self.one.is_some() || !self.items.is_empty()
    }
}

/// `render_edit`'s result: the text and op the editor takes.
pub struct Rendered {
    pub text: Vec<u8>,
    pub op: EditOp,
}

/// `text`/`op` with `value` rendered into `format` in place of the argument
/// text: the edit's own text for `one`, each `SetSequence` item for `items`.
/// The same for every format — a JSON file gets `5` for the number and
/// `"hello"` for the string, where it used to get every argument quoted.
/// A string `replace` or `set` becomes `ReplaceString` or `SetString`, so
/// the string it replaces keeps its quote style; a `--raw` value is the
/// user's own spelling, and stays as it is.
pub fn render_edit(text: &[u8], value: &OpValue, op: &EditOp, format: Format, layout: Layout) -> Result<Rendered> {
    let text_out = match &value.one {
        Some(v) => value_arg::render(v, format, layout)?,
        None => text.to_vec(),
    };
    let op_out: EditOp = match op {
        EditOp::SetSequence(items) => {
            if value.items.is_empty() {
                op.clone()
            } else {
                debug_assert_eq!(value.items.len(), items.len());
                let mut rendered = Vec::with_capacity(value.items.len());
                for v in &value.items {
                    rendered.push(value_arg::render(v, format, layout)?);
                }
                EditOp::SetSequence(rendered)
            }
        }
        EditOp::ReplaceValue => match string_of(value) {
            Some(s) => EditOp::ReplaceString(s),
            None => op.clone(),
        },
        EditOp::Set => match string_of(value) {
            Some(s) => EditOp::SetString(s),
            None => op.clone(),
        },
        _ => op.clone(),
    };
    Ok(Rendered { text: text_out, op: op_out })
}

/// The string `value` holds, when it is one string fig read and renders.
fn string_of(value: &OpValue) -> Option<Vec<u8>> {
    match value.one.as_ref()? {
        Value::Raw(_) => None,
        Value::Tree(t) => match t.node(t.root).kind {
            NodeKind::String(s) => Some(s.to_vec()),
            _ => None,
        },
    }
}

/// The minimal valid empty document for `format`, used to seed a file `set`
/// creates from scratch before landing its first key. `None` means the format
/// has no empty-document form to seed into, so a from-scratch `set` on it is
/// refused before any file is created — the two non-`Language` projections,
/// canonical and gron.
///
/// Every seed comes from the format registry's `empty_doc_seed`.
pub fn empty_doc_seed(format: Format) -> Option<&'static [u8]> {
    match format {
        // The canonical oracle grammar is a parse/print pair and gron is a
        // projection; neither is a stored format anyone creates from scratch.
        Format::CANONICAL | Format::GRON => None,
        _ => types::runtime_entry(format).and_then(|e| e.empty_doc_seed),
    }
}

/// What the CLI wants done to a document, independent of which language it
/// turns out to be — the payload of `route` below.
pub enum EditRequest<'a> {
    /// Splice an edit into the file in place. `text`/`op` are exactly what
    /// `apply_to_file` takes.
    Apply { path: &'a [PathSeg], text: &'a [u8], value: &'a OpValue, op: &'a EditOp },
    /// Read one comment back without writing (`comment --get`).
    GetComment { path: &'a [PathSeg], inline_comment: bool },
}

/// THE editor dispatch: the one place a CLI `Format` becomes "which language,
/// which dialect, and is this format editable at all". Every editing entry
/// point in the CLI goes through it — `replace`'s value and `rename`'s key,
/// `set`/`insert`/`delete`'s structural ops (via `apply_to_file_as`), and
/// both halves of `comment` — so the per-format knowledge below is stated once.
///
/// Returns the comment for a `GetComment` request (`None` when there is none)
/// and always `None` for an `Apply`.
///
/// The three per-format facts, all derived:
///
///   * WHETHER the format can be edited at all — `caps.edit`.
///   * HOW a value argument is spelled — `render_edit`, which renders the
///     value read from the command line into this format as a binding's
///     value is rendered (`value_arg::render`). Text with no value behind it
///     — a comment, a key name, a `--raw` argument — is spliced as it stands.
///   * WHICH dialect to reparse under — `Entry.dialect`, which is what keeps a
///     JSONC/JSON5 file's comments valid on reparse.
pub fn route(file: &mut Input, format: Format, req: EditRequest<'_>) -> Result<Option<Vec<u8>>> {
    match format {
        // The canonical form is a parse/print pair with no span-splicing
        // editor; convert via `get` instead of editing in place.
        Format::CANONICAL => Err(err!(UnsupportedCanonicalEdit)),
        // gron is a CLI-only get/echo projection with no in-place editor.
        Format::GRON => Err(err!(UnsupportedGronEdit)),
        // A registered language: the one `Editor`, with the entry's dialect
        // as its `format`; whether it edits at all and how it takes text are
        // the entry's own facts, read at the call.
        _ => {
            let e = types::runtime_entry(format).ok_or(err!(FormatDisabled))?;
            if !e.language.caps.edit {
                return Err(err!(FormatNotEditable));
            }
            match req {
                EditRequest::GetComment { path, inline_comment } => get_comment_from_file(&e, file, path, inline_comment),
                EditRequest::Apply { path, text, value, op } => {
                    apply_to_file(&e, file, path, text, value, op, format)?;
                    Ok(None)
                }
            }
        }
    }
}

// `FormatNotEditable` is the refusal for a language whose `caps.edit` is
// false. No language in tree declares that any more, so the arm above is
// unreachable in every shipped build; it stays because `caps.edit` is part
// of the `Language` contract an out-of-tree format may write against.

/// Apply an in-place edit to `file` as `format`. The wrapper every writing
/// action uses; see `route` for what it derives.
pub fn apply_to_file_as(file: &mut Input, format: Format, path: &[PathSeg], text: &[u8], value: &OpValue, op: &EditOp) -> Result<()> {
    route(file, format, EditRequest::Apply { path, text, value, op })?;
    Ok(())
}

/// Read one comment back from `file` as `format` without writing — the
/// `comment --get` wrapper around `route`.
pub fn get_comment_as(file: &mut Input, format: Format, path: &[PathSeg], inline_comment: bool) -> Result<Option<Vec<u8>>> {
    route(file, format, EditRequest::GetComment { path, inline_comment })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fig_core::registry::global;

    fn entry(name: &str) -> Option<std::sync::Arc<Entry>> {
        global().entry_by_name(name)
    }

    fn key(k: &str) -> PathSeg {
        PathSeg::Key(k.as_bytes().to_vec())
    }

    #[test]
    fn apply_edit_reads_end_before_the_end_of_a_path_as_the_last_item() {
        let Some(y) = entry("yaml") else { return };
        let item = [key("l"), PathSeg::Index(types::APPEND_INDEX)];
        assert_eq!(apply_edit(&y, b"l: [a, b]\n", &item, b"z", &EditOp::ReplaceValue).unwrap(), b"l: [a, z]\n");
        let nested = [key("o"), PathSeg::Index(types::APPEND_INDEX), key("n")];
        assert_eq!(apply_edit(&y, b"o:\n  - n: a\n  - n: b\n", &nested, b"c", &EditOp::ReplaceValue).unwrap(), b"o:\n  - n: a\n  - n: c\n");
        assert_eq!(apply_edit(&y, b"l: []\n", &item, b"z", &EditOp::ReplaceValue).unwrap_err(), err!(NotFound));
    }

    #[test]
    fn apply_edit_blames_the_spliced_text_not_the_file() {
        let Some(t) = entry("toml") else { return };
        let p = [key("rev")];
        assert_eq!(apply_edit(&t, b"rev = \"old\"\n", &p, b"cc5e7e51", &EditOp::ReplaceValue).unwrap_err(), err!(InvalidEditText));
        let q = [key("new")];
        assert_eq!(apply_edit(&t, b"rev = \"old\"\n", &q, b"cc5e7e51", &EditOp::Set).unwrap_err(), err!(InvalidEditText));
        let missing = [key("nope")];
        assert_eq!(apply_edit(&t, b"rev = \"old\"\n", &missing, b"\"x\"", &EditOp::ReplaceValue).unwrap_err(), err!(NotFound));
        assert_eq!(apply_edit(&t, b"rev = \"old\"\n", &p, b"\"cc5e7e51\"", &EditOp::ReplaceValue).unwrap(), b"rev = \"cc5e7e51\"\n");
    }

    #[test]
    fn apply_edit_performs_the_structural_ops_on_dotenv() {
        let Some(d) = entry("dotenv") else { return };
        assert_eq!(apply_edit(&d, b"", &[], b"bar", &EditOp::InsertKey(b"FOO".to_vec())).unwrap(), b"FOO=bar\n");
        assert_eq!(apply_edit(&d, b"FOO=bar\n", &[], b"qux", &EditOp::InsertKey(b"BAZ".to_vec())).unwrap(), b"FOO=bar\nBAZ=qux\n");
        assert_eq!(apply_edit(&d, b"FOO=bar\n", &[key("FOO")], b"", &EditOp::DeleteKey).unwrap(), b"");
    }

    #[test]
    fn empty_doc_seed_is_the_byte_string_the_registry_declares() {
        assert_eq!(empty_doc_seed(types::must_named("json")), Some(&b"{}\n"[..]));
        assert_eq!(empty_doc_seed(types::must_named("toml")), Some(&b""[..]));
        assert_eq!(empty_doc_seed(Format::CANONICAL), None);
        assert_eq!(empty_doc_seed(Format::GRON), None);
    }
}
