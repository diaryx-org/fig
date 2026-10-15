//! Port of src/cli/reformat.zig.
//!
//! The `fmt` and `convert` actions' cores: parse a slice under one format and
//! re-emit it, either in the same format (`reformat_slice`) or a different one
//! (`convert_slice`), sharing `get`'s parse-error/authoring-warning reporting
//! and lossy-conversion diagnostics so both actions' stderr output matches
//! `get`'s for the same file.

use std::borrow::Cow;

use fig_core::kit::ast::{SerializeOptions, AST};
use fig_core::lossless;
use fig_core::materialize::TagMode;
use fig_core::serialize::{self, Target};
use fig_core::{err, Result};

use crate::fileio::Terminal;
use crate::types::{self, Format};
use crate::{diag_report, parse_dispatch};

/// Parse `content` as `format` and re-emit it in the *same* format — the `fmt`
/// action's core, and `get`'s twin minus the cross-format machinery: since the
/// output format always equals the input, there's no YAML reference-layer
/// materialization and no `--lossless` envelope pass to consider (those only
/// matter when `from != to`). Returns the reformatted bytes.
#[allow(clippy::too_many_arguments)]
pub fn reformat_slice(
    term: &mut Terminal,
    file_path: &str,
    format: Format,
    content: &[u8],
    serialize: &SerializeOptions,
    quiet: bool,
    strict: bool,
) -> Result<Vec<u8>> {
    if format == Format::GRON {
        term.write_str("error: cannot format gron (a CLI projection, not a stored document format).\n")?;
        term.flush()?;
        return Err(err!(UnsupportedGronFmt));
    }

    let mut reports = parse_dispatch::Reports::default();
    let doc = match parse_dispatch::parse_slice_as(format, content, &mut reports) {
        Ok(d) => d,
        Err(e) => {
            reports.report_diagnostics(term, content, file_path)?;
            return Err(e);
        }
    };
    reports.report_warnings(term, content, file_path, quiet, strict)?;

    let diag_target = types::to_target(format).expect("gron rejected up front above");
    parse_dispatch::report_loss(term, &doc.ast, doc.ast.root, &diag_target, serialize, false, quiet, strict)?;
    print_to(term, &diag_target, &doc.ast, serialize, false)
}

/// Parse `content` as `from` and re-emit it as `to` — `convert`'s core, and the
/// in-place-writeback twin of `get`'s cross-format pipeline: materializing
/// YAML's reference layer when leaving YAML (aliases/merges/tags resolved),
/// the optional `--lossless` envelope round-trip, the same lossy-conversion
/// diagnostics, and TOML's lossy null-strip when serializing without
/// `--lossless`. Unlike `get` there is no path/gron projection — `convert`
/// always converts the whole document. Returns the converted bytes.
#[allow(clippy::too_many_arguments)]
pub fn convert_slice(
    term: &mut Terminal,
    file_path: &str,
    from: Format,
    to: Format,
    content: &[u8],
    serialize: &SerializeOptions,
    lossless: bool,
    lax_tags: bool,
    quiet: bool,
    strict: bool,
) -> Result<Vec<u8>> {
    if to == Format::GRON {
        term.write_str("error: cannot convert to gron (a CLI projection, not a stored document format).\n")?;
        term.flush()?;
        return Err(err!(UnsupportedGronFmt));
    }

    let mut reports = parse_dispatch::Reports::default();
    let doc = match parse_dispatch::parse_slice_as(from, content, &mut reports) {
        Ok(d) => d,
        Err(e) => {
            reports.report_diagnostics(term, content, file_path)?;
            return Err(e);
        }
    };
    reports.report_warnings(term, content, file_path, quiet, strict)?;

    // Leaving a language with a reference layer for one without resolves
    // the layer first (aliases → copies, merges → flattened, tags
    // applied/dropped); YAML→YAML keeps it intact for round-trip. Mirrors
    // `get`.
    let keeps_references = parse_dispatch::carries_references(from) && parse_dispatch::carries_references(to);
    let base_ast = parse_dispatch::materialize_for(term, from, to, &doc, file_path, if lax_tags { TagMode::Lax } else { TagMode::Strict })?;

    let ast: Cow<'_, AST> = if lossless && !keeps_references {
        // `to` is never gron here (rejected up front above); its language
        // declares what it holds — see `manifest::Caps.lossless` for the
        // per-format rationale (JSON5 reuse, canonical/fig decode-only,
        // INI/dotenv/properties/plist/NestedText's lack of an envelope).
        let maybe_native = parse_dispatch::native_for_format(to);
        let decoded = lossless::decode(&base_ast)?;
        match maybe_native {
            None => Cow::Owned(decoded),
            Some(native) => Cow::Owned(lossless::encode(&decoded, native)?),
        }
    } else {
        base_ast
    };

    let diag_target = types::to_target(to).expect("gron rejected up front above");
    parse_dispatch::report_loss(term, &ast, ast.root, &diag_target, serialize, lossless, quiet, strict)?;
    print_to(term, &diag_target, &ast, serialize, lossless)
}

/// Print `ast` whole as `target`: the oracle form as it stands, a language
/// through its vtable after the lossy strips its declarations ask for
/// (`parse_dispatch::print_runtime`). A refusal is reported and exits.
fn print_to(term: &mut Terminal, target: &Target, ast: &AST, serialize: &SerializeOptions, lossless: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match target {
        Target::Canonical => {
            let r = if parse_dispatch::CANONICAL_ENABLED {
                serialize::serialize_with(ast, &mut out, &Target::Canonical, serialize)
            } else {
                Err(err!(FormatDisabled))
            };
            if let Err(e) = r {
                diag_report::report_serialize_error(term, e);
            }
        }
        Target::Language(e) => {
            if let Err(x) = parse_dispatch::print_runtime(&mut out, e, ast, ast.root, serialize, lossless) {
                diag_report::report_runtime_print_error(term, x);
            }
        }
    }
    Ok(out)
}
