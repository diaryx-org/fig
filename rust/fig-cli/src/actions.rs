//! Port of src/cli/actions.zig.
//!
//! The body of each CLI action — one function per `CliAction`, called from
//! `main.rs`'s thin dispatch switch. Each function takes the stdout/stderr
//! terminals and its own action's option struct (from `types.rs`); together
//! they're exactly the free variables the original monolithic `main()`
//! switch closed over.

use std::borrow::Cow;

use fig_core::kit::ast::{NodeKind, PathSegment, SerializeOptions, AST};
use fig_core::kit::document::Document;
use fig_core::lossless;
use fig_core::materialize::TagMode;
use fig_core::serialize::{self, Target};
use fig_core::{err, Result};

use crate::edit_ops::{self, OpValue};
use crate::fileio::{self, Color, Input, OpenMode, Terminal};
use crate::help::{self, Help};
use crate::types::{self, *};
use crate::value_arg;
use crate::{args, diag_report, diff, external, gron, languages, parse_dispatch, patch_ops, reformat};

pub fn run_help(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &HelpOptions) -> Result<()> {
    if opts.oneline {
        stdout_term.write_str(help::ONELINE)?;
        stdout_term.flush()?;
        return Ok(());
    }
    stderr_term.write_str(help::TITLE_STRING)?;
    Help::general(stderr_term, binary_name)
}

/// The git-style fallback for a word that is not one of fig's own actions:
/// hand it to `fig-<word>` on PATH. All of it lives in `external.rs`; this
/// exists so that `main`'s dispatch switch stays one arm per action, straight
/// into this file.
pub fn run_external(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &ExternalOptions) -> Result<()> {
    external::run(stdout_term, stderr_term, binary_name, opts)
}

/// Print fig's one version — every artifact ships under it (see
/// docs/VERSIONING.md) — and its marketing epoch, a purely cosmetic label with
/// no compatibility meaning: `fig 5.0.0 "Texas Everbearing"`.
pub fn run_version(stdout_term: &mut Terminal, binary_name: &str, version: &str, epoch: &str, opts: &VersionOptions) -> Result<()> {
    if opts.requested_help {
        return Help::version(stdout_term, binary_name);
    }
    stdout_term.print(format_args!("fig {version} \"{epoch}\"\n"))?;
    stdout_term.flush()
}

fn exit_with(term: &mut Terminal, message: &str, code: i32) -> ! {
    let _ = term.write_str(message);
    let _ = term.flush();
    fileio::exit(code);
}

pub fn run_edit(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &EditOptions) -> Result<()> {
    if opts.requested_help {
        return if opts.key { Help::rename(stdout_term, binary_name) } else { Help::replace(stdout_term, binary_name) };
    }
    // A replacement VALUE is read before the file is touched, so one that is
    // not a value exits 2 with the file as it was; a replacement key is a
    // name, spelled by the editor.
    let value = if opts.key {
        OpValue::default()
    } else {
        OpValue { one: Some(value_arg::read(stderr_term, &opts.replacement, opts.value_mode)?), items: Vec::new() }
    };
    let mut input = fileio::get_input(Some(&opts.file), OpenMode::ReadWrite)?;

    let op = if opts.key { EditOp::ReplaceKey } else { EditOp::ReplaceValue };
    // Value/key replacement is the same span splice for every editable format
    // — a value or key node has a tight, contiguous span, so the generic
    // editor handles even a TOML table assembled from scattered headers — so
    // it routes through the shared editor dispatch like every other edit. That
    // is also where the value is spelled in the file's format, and where the
    // refusal of a read-only format and the canonical/gron refusals live; see
    // `edit_ops::route`.
    let resolved = if opts.detect { parse_dispatch::detect_file_format(&opts.file)? } else { opts.format };
    edit_ops::apply_to_file_as(&mut input, resolved, &opts.path, opts.replacement.as_bytes(), &value, &op)
}

pub fn run_set(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &SetOptions) -> Result<()> {
    if opts.requested_help {
        return Help::set(stdout_term, binary_name);
    }
    if opts.path.is_empty() {
        exit_with(stderr_term, "error: set needs a path to the key (or, with --seq, the sequence) to upsert.\n", 2);
    }
    // Read before any file is created or opened: see `run_edit`.
    let value = if opts.seq {
        let mut items = Vec::with_capacity(opts.values.len());
        for text in &opts.values {
            items.push(value_arg::read(stderr_term, text, opts.value_mode)?);
        }
        OpValue { one: None, items }
    } else {
        OpValue { one: Some(value_arg::read(stderr_term, &opts.value, opts.value_mode)?), items: Vec::new() }
    };
    // `set` upserts, so it may target a file that doesn't exist yet:
    // create it and seed a minimal valid empty document (see
    // `create_seeded_file`/`empty_doc_seed`) so the editor lands the first
    // key into a parseable buffer. The other structural actions keep the
    // plain open — they edit what's already there and have nothing to
    // seed into a blank file. Two cases refuse the create up front,
    // before any file lands on disk:
    //   - a freshly created file is empty, so its format can't be sniffed
    //     (`--detect`): it must carry a known extension;
    //   - a format with no empty-document form (`empty_doc_seed` == None)
    //     has nothing valid to seed.
    let mut created = false;
    let mut input = match fileio::get_input(Some(&opts.file), OpenMode::ReadWrite) {
        Ok(i) => i,
        Err(e) if e.is("FileNotFound") => {
            if opts.file == "-" {
                return Err(e);
            }
            if opts.detect {
                exit_with(
                    stderr_term,
                    &format!(
                        "error: cannot create {}: an unrecognized extension gives no format to seed. Use a known extension (.json/.jsonc/.yaml/.toml/.zon) or an existing file.\n",
                        opts.file
                    ),
                    2,
                );
            }
            let Some(seed) = edit_ops::empty_doc_seed(opts.format) else {
                exit_with(
                    stderr_term,
                    &format!(
                        "error: cannot create {}: {} has no empty-document form to seed a new file. Start from an existing file.\n",
                        opts.file,
                        types::name(opts.format)
                    ),
                    2,
                );
            };
            created = true;
            fileio::create_seeded_file(&opts.file, seed)?
        }
        Err(e) => return Err(e),
    };

    let resolved = if opts.detect { parse_dispatch::detect_file_format(&opts.file)? } else { opts.format };
    // `--seq` reconciles the sequence at `path`; otherwise upsert a scalar.
    // Both flow through the shared structural-edit router.
    // A path ending in an index upserts a sequence item: `[-]` appends, the
    // way it does for `insert`, starting the list when it is not there as
    // `set` starts a missing map, and an index that is there is replaced.
    // An index past the end is refused below, since there is no item
    // between the last and it to fill.
    let mut path: &[PathSeg] = &opts.path;
    let item: Option<usize> = if opts.seq {
        None
    } else {
        match path[path.len() - 1] {
            PathSeg::Index(index) => Some(index),
            PathSeg::Key(_) => None,
        }
    };
    let op = if opts.seq {
        EditOp::SetSequence(opts.values.iter().map(|v| v.as_bytes().to_vec()).collect())
    } else if item == Some(APPEND_INDEX) {
        path = &path[..path.len() - 1];
        EditOp::SetAppend
    } else if item.is_some() {
        EditOp::ReplaceValue
    } else {
        EditOp::Set
    };
    let text: &[u8] = if opts.seq { b"" } else { opts.value.as_bytes() };
    // On a from-scratch create, roll the new file back if the edit fails
    // (e.g. a path whose parent segment resolves to a scalar, which `set`
    // refuses rather than clobber) so a failed `set` leaves no bare seed
    // behind — matching how it leaves an existing file untouched on
    // failure. A merely-missing parent map is no longer a failure: `set`
    // auto-vivifies it (see `Editor.set`).
    if let Err(err) = edit_ops::apply_to_file_as(&mut input, resolved, path, text, &value, &op) {
        drop(input);
        if created {
            fileio::delete_created_file(&opts.file);
        }
        if err.is("NotFound") && op == EditOp::ReplaceValue {
            exit_with(
                stderr_term,
                &format!(
                    "error: there is no item [{}] to set, so nothing was written; set replaces an item that is there, or appends one with [-].\n",
                    item.unwrap()
                ),
                1,
            );
        }
        // `set` starts a missing list from the format's empty one; a format
        // with none has nothing to start it with, which the lookup's "no
        // such path" would not say.
        if err.is("NotFound") && (op == EditOp::SetAppend || matches!(op, EditOp::SetSequence(_))) && !starts_lists(resolved) {
            exit_with(
                stderr_term,
                &format!(
                    "error: {} has no spelling for an empty list, so set cannot start one; nothing was written.\n",
                    types::name(resolved)
                ),
                1,
            );
        }
        return Err(err);
    }
    Ok(())
}

/// Whether `format` spells an empty sequence (`Syntax.empty_seq_literal`),
/// so that `set` can start a list that is not there.
fn starts_lists(format: Format) -> bool {
    let Some(e) = types::runtime_entry(format) else { return false };
    e.syntax_or_default().empty_seq_literal.is_some()
}

pub fn run_insert(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &InsertOptions) -> Result<()> {
    if opts.requested_help {
        return Help::insert(stdout_term, binary_name);
    }
    // Read before the file is opened: see `run_edit`.
    let value = OpValue { one: Some(value_arg::read(stderr_term, &opts.value, opts.value_mode)?), items: Vec::new() };
    let mut input = fileio::get_input(Some(&opts.file), OpenMode::ReadWrite)?;

    // The destination is the *parent* container plus the trailing slot.
    // A trailing key inserts into a mapping; a trailing index pre/appends
    // to a sequence. An empty parent is the root container.
    if opts.path.is_empty() {
        exit_with(stderr_term, "error: insert needs a destination path (e.g. a.b.newkey or list[-]).\n", 2);
    }
    let parent = &opts.path[..opts.path.len() - 1];
    let resolved = if opts.detect { parse_dispatch::detect_file_format(&opts.file)? } else { opts.format };
    match &opts.path[opts.path.len() - 1] {
        PathSeg::Key(key) => {
            edit_ops::apply_to_file_as(&mut input, resolved, parent, opts.value.as_bytes(), &value, &EditOp::InsertKey(key.clone()))
        }
        PathSeg::Index(index) => {
            // `[0]` prepends, `[-]`/`[$]` appends, and any other index
            // inserts before the item there; past the end is the editor's
            // `NotFound`, reported as every refused edit is.
            let op = if *index == 0 {
                EditOp::PrependSeq
            } else if *index == APPEND_INDEX {
                EditOp::AppendSeq
            } else {
                EditOp::InsertSeqItem(*index)
            };
            match edit_ops::apply_to_file_as(&mut input, resolved, parent, opts.value.as_bytes(), &value, &op) {
                Ok(()) => Ok(()),
                Err(err) => {
                    // `insert` adds to a list that is there, as it adds a key to
                    // a map that is; `set` is the verb that starts one.
                    if err.is("NotFound") && !matches!(op, EditOp::InsertSeqItem(_)) {
                        exit_with(
                            stderr_term,
                            &format!(
                                "error: there is no list to insert into, so nothing was written; `{binary_name} set` starts one: `{binary_name} set {} <path>[-] <value>`.\n",
                                opts.file
                            ),
                            1,
                        );
                    }
                    Err(err)
                }
            }
        }
    }
}

pub fn run_delete(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &DeleteOptions) -> Result<()> {
    if opts.requested_help {
        return Help::delete(stdout_term, binary_name);
    }
    let mut input = fileio::get_input(Some(&opts.file), OpenMode::ReadWrite)?;

    if opts.path.is_empty() {
        exit_with(stderr_term, "error: delete needs a path to the entry or item to remove.\n", 2);
    }
    let resolved = if opts.detect { parse_dispatch::detect_file_format(&opts.file)? } else { opts.format };
    // A trailing index removes that item from the parent sequence; a
    // trailing key deletes the mapping entry named by the full path.
    let none = OpValue::default();
    match &opts.path[opts.path.len() - 1] {
        PathSeg::Index(index) => edit_ops::apply_to_file_as(
            &mut input,
            resolved,
            &opts.path[..opts.path.len() - 1],
            b"",
            &none,
            &EditOp::RemoveSeqItem(*index),
        ),
        PathSeg::Key(_) => edit_ops::apply_to_file_as(&mut input, resolved, &opts.path, b"", &none, &EditOp::DeleteKey),
    }
}

pub fn run_get(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &GetOptions) -> Result<()> {
    if opts.requested_help {
        return Help::get(stdout_term, binary_name);
    }
    let mut input = fileio::get_input(Some(&opts.file), OpenMode::ReadOnly)?;

    // Resolved input/output formats. They equal the parsed options unless
    // the input format has to be sniffed from the file's contents (no
    // `--input`, unrecognized extension): detection overwrites `from`, and
    // — when no `--output` was given — `to` follows it (an echo round-trip
    // rather than a silent convert-to-JSON).
    let mut from = opts.from;
    let mut to = opts.to;

    // Read once so detection and parsing share the same bytes — a
    // piped stdin can only be consumed a single time.
    let content = fileio::read_all(&mut input)?;
    if opts.detect {
        from = parse_dispatch::resolve_format_from_content(&content, &opts.file)?;
        if !opts.output_explicit {
            to = from;
        }
    }
    let mut reports = parse_dispatch::Reports::default();
    let doc: Document = match parse_dispatch::parse_slice_as(from, &content, &mut reports) {
        Ok(d) => d,
        Err(e) => {
            // A parse failure renders as a `file:line:col` teaching
            // message (DESIGN.md: every diagnostic names the fix) and
            // exits cleanly — no error-return trace for a user typo.
            reports.report_diagnostics(stderr_term, &content, &opts.file)?;
            return Err(e);
        }
    };
    // Authoring-time lints (parse-time warnings) ride the same
    // `--quiet`/`--strict` contract as the serialize-side diagnostics
    // below: quiet silences, strict aborts.
    reports.report_warnings(stderr_term, &content, &opts.file, opts.quiet, opts.strict)?;

    // Leaving a language with a reference layer for one without resolves
    // the layer first (aliases → copies, merges → flattened, tags
    // applied/dropped). YAML→YAML keeps it intact for round-trip; JSON
    // never has it. Each side is asked, not named (`Caps.references`).
    let keeps_references = parse_dispatch::carries_references(from) && parse_dispatch::carries_references(to);
    let base_ast = parse_dispatch::materialize_for(stderr_term, from, to, &doc, &opts.file, if opts.lax_tags { TagMode::Lax } else { TagMode::Strict })?;

    // Lossless mode: decode any `$fig` envelopes in the input back to
    // their real node kinds, then re-encode for the target format. Skipped
    // when the reference layer is kept (YAML→YAML): it lives in side-tables
    // the core-AST passes would strip — and it round-trips losslessly
    // already.
    let ast: Cow<'_, AST> = if opts.lossless && !keeps_references {
        // gron is CLI-only — it has no serialize target of its own, so no
        // `caps.lossless` — but its value layer is JSON, so it encodes for
        // JSON's declaration.
        let maybe_native = parse_dispatch::native_for_format(to);
        let decoded = lossless::decode(&base_ast)?;
        match maybe_native {
            None => Cow::Owned(decoded),
            Some(native) => Cow::Owned(lossless::encode(&decoded, native)?),
        }
    } else {
        base_ast
    };

    let node_id = match &opts.path {
        Some(p) => {
            let resolved = edit_ops::resolve_last(&ast, p);
            ast.get_val_by_path(&segs(&resolved))?.id
        }
        None => ast.root,
    };

    // A scalar at a path prints as its text and a newline, the way `jq -r`
    // does, whatever the file's format: a string unquoted, anything else as
    // fig spells it (`42`, `true`, `null`, a datetime as written). `-o`
    // asks for a format's own spelling of the fragment and keeps getting it
    // (`fig get f name -o json` → `"hi"`). CLI 5 §2.
    if opts.path.is_some() && !opts.output_explicit {
        if let Some(text) = scalar_text(&ast, node_id)? {
            stdout_term.write_all(&text)?;
            stdout_term.write_byte(b'\n')?;
            stdout_term.flush()?;
            return Ok(());
        }
    }

    // gron is a CLI-only projection that derives straight from the AST,
    // so it has no serialize target: print it here and return, bypassing
    // the serializer dispatch and the lossy/lossless diagnostics below.
    // Aliases are already materialized above.
    if to == Format::GRON {
        let mut out = Vec::new();
        gron::print_node(&mut out, &ast, node_id, &opts.gron_projection)?;
        stdout_term.write_all(&out)?;
        stdout_term.flush()?;
        return Ok(());
    }

    // Surface everything the conversion would silently lose (comments
    // dropped/degraded, values dropped/degraded) — unless `--quiet`;
    // `--strict` turns any warning into a hard failure. A runtime target is
    // held to what its declaration says it holds.
    let diag_target = types::to_target(to).expect("gron returned above");
    parse_dispatch::report_loss(stderr_term, &ast, node_id, &diag_target, &opts.serialize, opts.lossless, opts.quiet, opts.strict)?;

    match &diag_target {
        // The oracle form holds every value; nothing to strip.
        Target::Canonical => {
            let mut out = Vec::new();
            let r = if parse_dispatch::CANONICAL_ENABLED {
                serialize::serialize_node_with(&ast, &mut out, &Target::Canonical, node_id, &opts.serialize)
            } else {
                Err(err!(FormatDisabled))
            };
            if let Err(e) = r {
                stdout_term.flush()?;
                diag_report::report_serialize_error(stderr_term, e);
            }
            stdout_term.write_all(&out)?;
        }
        // A language prints through its vtable, after the lossy strips its
        // declarations ask for. A scalar comes back as the fragment the
        // editor would splice — the value as it stands alone, no newline —
        // so `get` ends the line here.
        Target::Language(e) => {
            let mut out = Vec::new();
            if let Err(x) = parse_dispatch::print_runtime(&mut out, e, &ast, node_id, &opts.serialize, opts.lossless) {
                stdout_term.flush()?;
                diag_report::report_runtime_print_error(stderr_term, x);
            }
            stdout_term.write_all(&out)?;
            let is_container = matches!(ast.node(node_id).kind, NodeKind::Mapping(_) | NodeKind::Sequence(_));
            if !is_container && !out.is_empty() && out[out.len() - 1] != b'\n' {
                stdout_term.write_byte(b'\n')?;
            }
        }
    }
    stdout_term.flush()
}

/// The text `get` prints for the scalar at `id`, or `None` for a container
/// (and for an unresolved alias, which prints as the format writes it).
fn scalar_text(ast: &AST, id: u32) -> Result<Option<Vec<u8>>> {
    match ast.node(id).kind {
        NodeKind::String(s) => return Ok(Some(s.to_vec())),
        NodeKind::Null | NodeKind::Boolean(_) | NodeKind::Number(_) | NodeKind::Extended(_) => {}
        NodeKind::Sequence(_) | NodeKind::Mapping(_) | NodeKind::KeyValue(_) | NodeKind::Alias(_) => return Ok(None),
    }
    // fig's spelling; JSON's otherwise, which agrees for every scalar but
    // a datetime (a JSON string, unquoted here).
    let has_fig = args::parse_format_name("figl").is_some();
    let target = Target::named(if has_fig { "figl" } else { "json" }).ok_or(err!(FormatDisabled))?;
    let mut view = ast.clone();
    view.root = id;
    let mut w = Vec::new();
    serialize::serialize_fragment_with(&view, &mut w, &target, &SerializeOptions::default())?;
    while w.last() == Some(&b'\n') {
        w.pop();
    }
    if !has_fig {
        if let NodeKind::Extended(e) = ast.node(id).kind {
            if !w.is_empty() && w[0] == b'"' {
                return Ok(Some(e.text.to_vec()));
            }
        }
    }
    Ok(Some(w))
}

pub fn run_comment(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &CommentOptions) -> Result<()> {
    if opts.requested_help {
        return Help::comment(stdout_term, binary_name);
    }
    // `--get` only reads: open read-only and never write back.
    let mut input = fileio::get_input(Some(&opts.file), if opts.get { OpenMode::ReadOnly } else { OpenMode::ReadWrite })?;

    let resolved = if opts.detect { parse_dispatch::detect_file_format(&opts.file)? } else { opts.format };

    if opts.get {
        // Strict JSON has no comment syntax: there can be nothing to get.
        // The one refusal the shared dispatch can't state, because it is
        // about a DIALECT of an otherwise comment-carrying language — and
        // it is a message-and-exit(1), not an error value.
        if types::is(resolved, "json") {
            exit_with(stderr_term, "error: strict JSON has no comments; use a .jsonc or .json5 file instead.\n", 1);
        }
        // Everything else reads its comment through the shared editor
        // dispatch. A format whose grammar has no same-line comment (INI,
        // NestedText) still answers a leading-comment read and surfaces
        // `CommentsUnsupported` from the editor under `--inline`.
        let comment = edit_ops::get_comment_as(&mut input, resolved, &opts.path, opts.inline_comment)?;
        // A missing comment is a missing path: nothing on stdout and exit 1,
        // the way `get` answers for a path that is not there, so "is there a
        // comment" is `if fig comment --get …`. A present-but-empty one
        // prints just its newline.
        let Some(text) = comment else {
            exit_with(
                stderr_term,
                &format!("error: no {}comment at that path\n", if opts.inline_comment { "inline " } else { "" }),
                1,
            );
        };
        stdout_term.write_all(&text)?;
        stdout_term.write_str("\n")?;
        return stdout_term.flush();
    }

    // Pick the op from the two flags: `--inline` selects the trailing
    // (same-line) comment vs the leading block; `--delete` removes it
    // rather than adding/setting. The marker (`#`, `//`) is the editor's
    // job.
    let op = if opts.delete {
        if opts.inline_comment { EditOp::DeleteTrailingComment } else { EditOp::DeleteLeadingComments }
    } else if opts.inline_comment {
        EditOp::SetTrailingComment
    } else {
        EditOp::AddLeadingComment
    };

    // Strict JSON has no comment syntax: fail with a clear message
    // rather than letting the editor surface a bare error. The same
    // refusal, and the same reason, as the `--get` branch above.
    if types::is(resolved, "json") {
        exit_with(stderr_term, "error: strict JSON has no comments; use a .jsonc or .json5 file instead.\n", 1);
    }
    // Everything else goes through the shared editor dispatch — JSONC/
    // JSON5's `//` comments reparse under their own dialect there, and
    // `--inline` set/delete on a format with no same-line comment syntax
    // surfaces `CommentsUnsupported` from the editor.
    edit_ops::apply_to_file_as(&mut input, resolved, &opts.path, opts.text.as_bytes(), &OpValue::default(), &op)
}

pub fn run_check(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &CheckOptions) -> Result<()> {
    if opts.requested_help {
        return Help::check(stdout_term, binary_name);
    }

    // Validate every file, reporting each independently, so one bad file
    // doesn't hide the status of the rest. Success lines go to stdout
    // (silenced by `--quiet`); failures always go to stderr. A single
    // bad file makes the whole run exit non-zero — the CI contract.
    let mut any_failed = false;
    for file in &opts.files {
        let mut diag_source: Option<Vec<u8>> = None;
        let mut diag_errors = None;
        let mut diag_warnings = None;
        match parse_dispatch::check_one(file, opts.format, opts.spec.as_deref(), &mut diag_source, &mut diag_errors, &mut diag_warnings) {
            Ok(fmt) => {
                let warned = diag_warnings.as_ref().map_or(0, |w: &Vec<_>| w.len());
                let source = diag_source.as_deref().unwrap_or(b"");
                if opts.strict && warned > 0 {
                    // `--strict`: the file parses, but a lint fails it like an
                    // error would — each warning, then why the file is not `ok`.
                    any_failed = true;
                    if !opts.quiet {
                        for w in diag_warnings.as_ref().unwrap() {
                            diag_report::print_diag(stderr_term, source, file, w.offset, w.end, "warning", Color::Yellow, &w.message, &w.short_label)?;
                        }
                    }
                    stderr_term.set_color(Color::Red)?;
                    stderr_term.write_str("error")?;
                    stderr_term.set_color(Color::Reset)?;
                    stderr_term.print(format_args!(": {file}: {warned} warning(s); --strict fails it\n"))?;
                    stderr_term.flush()?;
                    continue;
                }
                if !opts.quiet {
                    stdout_term.set_color(Color::Green)?;
                    stdout_term.write_str("ok")?;
                    stdout_term.set_color(Color::Reset)?;
                    // Echo the pinned version alongside the format when one
                    // was requested, so `ok` states exactly what was checked.
                    match &opts.spec {
                        Some(spec) => stdout_term.print(format_args!(": {file} ({} {spec})\n", types::name(fmt)))?,
                        None => stdout_term.print(format_args!(": {file} ({})\n", types::name(fmt)))?,
                    }
                    // Authoring-time lints: the file is valid (still `ok`),
                    // but likely-mistake lines print right below it.
                    if let Some(ws) = &diag_warnings {
                        stdout_term.flush()?;
                        for w in ws {
                            diag_report::print_diag(stderr_term, source, file, w.offset, w.end, "warning", Color::Yellow, &w.message, &w.short_label)?;
                        }
                        stderr_term.flush()?;
                    }
                }
            }
            Err(err) => {
                any_failed = true;
                // A covered-language failure renders as a full
                // `file:line:col: error: …` teaching report per error
                // (recovery collects every error in the file, not just the
                // first) instead of the generic `file: ErrorName` line.
                if let Some(errs) = &diag_errors {
                    let source = diag_source.as_deref().unwrap_or(b"");
                    for d in errs {
                        diag_report::print_diag(stderr_term, source, file, d.offset, d.end, "error", Color::Red, &d.message, &d.short_label)?;
                    }
                    stderr_term.flush()?;
                    continue;
                }
                stderr_term.set_color(Color::Red)?;
                stderr_term.write_str("error")?;
                stderr_term.set_color(Color::Reset)?;
                if err.is("UnsupportedSpec") {
                    // A spec mismatch is a CLI usage error, not a malformed
                    // document — say so plainly with the offending version.
                    stderr_term.print(format_args!(
                        ": {file}: --spec '{}' is not valid for this format\n",
                        opts.spec.as_deref().unwrap_or("")
                    ))?;
                } else {
                    stderr_term.print(format_args!(": {file}: {}\n", err.name()))?;
                }
            }
        }
    }
    stdout_term.flush()?;
    stderr_term.flush()?;
    if any_failed {
        fileio::exit(1);
    }
    Ok(())
}

/// `fig patch`: merge one document into another, in place.
///
/// The order below is not arbitrary. The TARGET is resolved first — read
/// and format-detected — because the patch document's own
/// preparation depends on it: `--lossless` encodes the patch's values for the
/// format they are about to be rendered into, which is the target's.
pub fn run_patch(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &PatchOptions) -> Result<()> {
    if opts.requested_help {
        return Help::patch(stdout_term, binary_name);
    }
    // `--dry-run`/`--diff` are preview modes: nothing is written under either,
    // so the target opens read-only and stdin becomes a legal target.
    let preview_only = opts.dry_run || opts.diff;
    let target_is_stdin = opts.file == "-";
    if !preview_only && target_is_stdin {
        exit_with(stderr_term, "error: cannot patch stdin in place; pass --dry-run or --diff to print the result instead.\n", 2);
    }

    let mut input = fileio::get_input(Some(&opts.file), if preview_only { OpenMode::ReadOnly } else { OpenMode::ReadWrite })?;
    let content = fileio::read_all(&mut input)?;

    // Sniffed from the bytes when the extension didn't say.
    let target_format = if opts.detect { parse_dispatch::resolve_format_from_content(&content, &opts.file)? } else { opts.format };

    let (patch_ast, patch_root) = load_patch(stderr_term, opts, target_format)?;
    let at = segs(&opts.at);
    let delete_segs: Vec<Vec<PathSegment<'_>>> = opts.deletes.iter().map(|d| segs(d)).collect();
    let deletes: Vec<&[PathSegment<'_>]> = delete_segs.iter().map(|d| d.as_slice()).collect();
    let req = patch_ops::Request { at: &at, patch: &patch_ast, from: patch_root, deletes: &deletes, options: opts.patch_options };

    let patched = patch_ops::apply_to_slice(&content, target_format, &req)?;

    // The one thing a silent success would hide: trivia the patch carried that
    // the target has nowhere to put (strict JSON has no comment syntax at all).
    // Said before anything is written, so `--strict` can refuse the result.
    if patched.stats.comments_dropped > 0 {
        if !opts.quiet {
            stderr_term.print(format_args!(
                "warning: dropped {} comment(s) the patch carried — {} has no place for them here.\n",
                patched.stats.comments_dropped, opts.file
            ))?;
        }
        if opts.strict {
            exit_with(stderr_term, &format!("error: {} was not patched; --strict aborts on a warning.\n", opts.file), 1);
        }
        stderr_term.flush()?;
    }

    let changed = content != patched.content;
    if opts.diff {
        let mut out = Vec::new();
        diff::unified_diff(&mut out, &opts.file, &content, &patched.content, 3);
        stdout_term.write_all(&out)?;
        stdout_term.flush()?;
    } else if opts.dry_run {
        stdout_term.write_all(&patched.content)?;
        stdout_term.flush()?;
    } else if changed {
        // Read-then-splice-same-handle, as in `fmt`: `content` was read before
        // this write, so there is no truncate-before-read race.
        input.rewrite(&patched.content)?;
    }
    Ok(())
}

/// Read, parse and prepare the patch document: the subtree `--from` names, in
/// a form that can be rendered into `target_format` without dangling.
///
/// Two preparation passes, both conditional and both about references rather
/// than values:
///
///   * A YAML patch that declares anchors is materialized (aliases → copies,
///     `<<` merges → flattened). Its reference layer is defined in the PATCH
///     file, which the target has never seen, so a `*name` spliced across
///     verbatim would point at nothing. Skipped when there are no anchors,
///     which is when materializing could only lose things (custom tags) and
///     gain nothing.
///   * `--lossless` decodes any `$fig` envelope the patch carries, then
///     re-encodes for the target's own type system — the same symmetric pair
///     `get`/`convert` run, and for the same reason.
fn load_patch(stderr_term: &mut Terminal, opts: &PatchOptions, target_format: Format) -> Result<(AST, u32)> {
    let mut file = fileio::get_input(Some(&opts.patch_file), OpenMode::ReadOnly)?;
    let content = fileio::read_all(&mut file)?;

    let format = if opts.detect_patch {
        parse_dispatch::resolve_format_from_content(&content, &opts.patch_file)?
    } else {
        opts.patch_format
    };

    let mut reports = parse_dispatch::Reports::default();
    let doc = match parse_dispatch::parse_slice_as(format, &content, &mut reports) {
        Ok(d) => d,
        Err(e) => {
            // Same contract as `get`'s: a parse failure in a file the user named
            // renders as a `file:line:col` teaching message, not an error name.
            reports.report_diagnostics(stderr_term, &content, &opts.patch_file)?;
            return Err(e);
        }
    };
    reports.report_warnings(stderr_term, &content, &opts.patch_file, opts.quiet, opts.strict)?;

    let mut ast = doc.ast;
    if ast.has_anchors() {
        // A patch must not carry unresolved aliases (see `patch.rs`).
        // Whichever language produced the anchors, the target has not seen
        // them, so the pass runs on the fact rather than the declaration.
        ast = parse_dispatch::materialize(&ast, TagMode::Lax)?;
    }

    let root = if opts.from.is_empty() { ast.root } else { ast.get_val_by_path(&segs(&opts.from))?.id };
    if !opts.lossless {
        return Ok((ast, root));
    }

    // Both envelope passes work from `ast.root`, so the subtree is selected
    // first, by re-rooting a copy: node ids are self-referential, so a copy
    // pointing elsewhere IS that subtree.
    let mut view = ast;
    view.root = root;
    let decoded = lossless::decode(&view)?;
    // gron encodes for JSON's declaration, as `run_get` does — its value
    // layer is JSON.
    let Some(native) = parse_dispatch::native_for_format(target_format) else {
        let r = decoded.root;
        return Ok((decoded, r));
    };
    let encoded = lossless::encode(&decoded, native)?;
    let r = encoded.root;
    Ok((encoded, r))
}

pub fn run_fmt(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &FmtOptions) -> Result<()> {
    if opts.requested_help {
        return Help::fmt(stdout_term, binary_name);
    }
    // `--diff` is a preview mode just like `--dry-run` — nothing is
    // ever written to `file` under either.
    let preview_only = opts.dry_run || opts.diff;
    let is_stdin = opts.file == "-";
    if !preview_only && is_stdin {
        exit_with(
            stderr_term,
            "error: cannot format stdin in place; pass --dry-run or --diff to print the formatted result instead.\n",
            2,
        );
    }

    // Read-only when only previewing (`--dry-run`/`--diff`): no need to
    // open for writing what will never be written. Otherwise read_write,
    // like `replace`/`set`/`insert`/`delete` — read the whole file first,
    // then splice the same handle in place (never via shell redirection,
    // which truncates a `> file` target before this process ever runs).
    let mut input = fileio::get_input(Some(&opts.file), if preview_only { OpenMode::ReadOnly } else { OpenMode::ReadWrite })?;

    let content = fileio::read_all(&mut input)?;

    let mut from = opts.from;
    if opts.detect {
        from = parse_dispatch::resolve_format_from_content(&content, &opts.file)?;
    }

    let reformatted = reformat::reformat_slice(stderr_term, &opts.file, from, &content, &opts.serialize, opts.quiet, opts.strict)?;
    let changed = content != reformatted;

    if opts.diff {
        let mut out = Vec::new();
        diff::unified_diff(&mut out, &opts.file, &content, &reformatted, 3);
        stdout_term.write_all(&out)?;
        stdout_term.flush()?;
        if changed {
            fileio::exit(1);
        }
    } else if opts.dry_run {
        stdout_term.write_all(&reformatted)?;
        stdout_term.flush()?;
        if changed {
            fileio::exit(1);
        }
    } else if changed {
        // Read-then-splice-same-handle (never shell redirection): the
        // in-memory `content` above was read before this write touches
        // the file, so there is no truncate-before-read race.
        input.rewrite(&reformatted)?;
    }
    Ok(())
}

/// Finish a `convert` invocation given the original `content` and the
/// `result` it converts to: write `result` back to `input` in place when
/// `write` is set (skipped if the bytes are already identical), then print
/// either a unified diff (`show_diff`) or the whole `result` to stdout — the
/// whole result only when neither `write` nor `show_diff` fired, so `--write`
/// alone stays silent (like `fmt`) and `--write --diff` shows what changed
/// without also dumping the full file.
#[allow(clippy::too_many_arguments)]
fn finish_convert(
    stdout_term: &mut Terminal,
    input: &mut Input,
    file_path: &str,
    content: &[u8],
    result: &[u8],
    write: bool,
    show_diff: bool,
) -> Result<()> {
    let changed = content != result;
    if write && changed {
        input.rewrite(result)?;
    }
    if show_diff {
        let mut out = Vec::new();
        diff::unified_diff(&mut out, file_path, content, result, 3);
        stdout_term.write_all(&out)?;
        stdout_term.flush()?;
    } else if !write {
        stdout_term.write_all(result)?;
        stdout_term.flush()?;
    }
    Ok(())
}

pub fn run_lang(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &LangOptions) -> Result<()> {
    if opts.requested_help {
        return Help::lang(stderr_term, binary_name);
    }
    match opts.verb {
        LangVerb::List => languages::list(stdout_term),
        LangVerb::Check => languages::check(stdout_term, stderr_term, &opts.name, opts.against.as_deref(), &opts.files),
        LangVerb::Table => languages::print_table(stdout_term, stderr_term, &opts.name, opts.input, opts.spec.as_deref()),
    }
}

pub fn run_convert(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &ConvertOptions) -> Result<()> {
    if opts.requested_help {
        return Help::convert(stdout_term, binary_name);
    }
    let is_stdin = opts.file == "-";
    if opts.write && is_stdin {
        exit_with(stderr_term, "error: cannot write stdin in place; omit --write to print the converted result instead.\n", 2);
    }

    let mut input = fileio::get_input(Some(&opts.file), if opts.write { OpenMode::ReadWrite } else { OpenMode::ReadOnly })?;

    let content = fileio::read_all(&mut input)?;

    let mut from = opts.from;
    if opts.detect {
        from = parse_dispatch::resolve_format_from_content(&content, &opts.file)?;
    }

    let converted = reformat::convert_slice(
        stderr_term,
        &opts.file,
        from,
        opts.to,
        &content,
        &opts.serialize,
        opts.lossless,
        opts.lax_tags,
        opts.quiet,
        opts.strict,
    )?;
    finish_convert(stdout_term, &mut input, &opts.file, &content, &converted, opts.write, opts.diff)
}

