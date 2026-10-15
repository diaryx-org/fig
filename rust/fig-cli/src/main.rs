//! Port of src/cli/main.zig.
//!
//! Main entrypoint for `fig` CLI
//! Design:
//! fig <action> [action options] [--flags]
//!
//! This file is deliberately thin: process/terminal setup, log routing,
//! and `parse_config` (clap, which reports a usage error itself), then a
//! dispatch switch straight into `actions.rs`. Everything else (arg parsing, format detection,
//! parse dispatch, in-place editing, reformat/convert, diagnostic rendering,
//! the `fig-<action>` handoff) lives in its own sibling module — see each
//! file's own doc comment.

use std::sync::OnceLock;

use fig_core::Error;

use crate::fileio::{Color, Mode, Sink, Terminal};
use crate::types::{CliActionOptions, CliConfig, ParseTarget};

/// The colour mode of standard error, set by `main` once it is known; the
/// log lines below are written in it.
static LOG_MODE: OnceLock<Mode> = OnceLock::new();

/// What reaches a user is written in the CLI's own voice — `error: …`,
/// `warning: …`, `note: …`, coloured like every diagnostic `diag_report`
/// prints — not a logger's `error(parseConfig): …`. A message's trailing
/// newline (several `parse_config` messages carry one) is dropped so each
/// message is one line. Debug lines exist only in a Zig Debug build, so a
/// release Rust build writes none.
pub fn log(level: &str, color: Color, message: &str) {
    let mode = LOG_MODE.get().copied().unwrap_or(Mode::NoColor);
    let mut t = Terminal::new(Sink::Stderr, mode);
    let body = message.trim_end_matches('\n');
    let _ = t.set_color(color);
    let _ = t.write_str(level);
    let _ = t.set_color(Color::Reset);
    let _ = t.print(format_args!(": {body}\n"));
}

/// `std.log.err`, in the CLI's voice (see `log`).
#[macro_export]
macro_rules! log_err {
    ($($arg:tt)*) => { $crate::log("error", $crate::fileio::Color::Red, &format!($($arg)*)) };
}

/// `std.log.warn`, in the CLI's voice (see `log`).
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::log("warning", $crate::fileio::Color::Yellow, &format!($($arg)*)) };
}

mod actions;
mod args;
mod diag_report;
mod diff;
mod edit_ops;
mod external;
mod fileio;
mod gron;
mod help;
mod host_languages;
mod languages;
mod parse_dispatch;
mod patch_ops;
mod reformat;
mod types;
mod value_arg;

// fig's one version — the core, the CLI, the Rust crates and both npm
// packages all ship under it (see docs/VERSIONING.md).
const VERSION: &str = env!("CARGO_PKG_VERSION");

// The current marketing epoch (`epoch` in build.zig) — purely cosmetic, no
// compatibility meaning; see `fig version`'s output.
const EPOCH: &str = "Texas Everbearing";

fn main() {
    // Respected environment variables
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let clicolor_force = std::env::var_os("CLICOLOR_FORCE").is_some();

    let mut stdout_terminal = Terminal::stdout(no_color, clicolor_force);
    let mut stderr_terminal = Terminal::stderr(no_color, clicolor_force);
    // From here on, log lines are written in standard error's mode.
    let _ = LOG_MODE.set(stderr_terminal.mode);

    // The languages are resolved while arguments are parsed (`--input
    // <name>`, an extension, `--lang`), so the registry is filled first.
    languages::init();

    // Accessing command line arguments:
    let argv = std::env::args_os().map(|a| a.into_string().unwrap_or_else(|a| a.to_string_lossy().into_owned()));

    // A usage error is clap's to report: it prints it with the action's
    // usage and exits 2 (or, for `--help`, prints the help and exits 0).
    let config = match args::parse_config(argv) {
        Ok(c) => c,
        Err(err) => err.exit(),
    };

    // Now, act on config. One failure is intercepted here rather than left to
    // escape as a bare `error: <ErrorName>`: an edit whose spliced TEXT is
    // what doesn't parse (`fig replace c.toml dep.rev cc5e7e51` — a git sha the
    // TOML parser can only read as a malformed number). `edit_ops::apply_edit`
    // tags that case; this is the only place that knows which argument the
    // text came from, so it is where the report is written. See
    // `diag_report::report_bad_edit_text`. An edit with no text of the caller's
    // (a delete) can fail the same way, when what it leaves does not parse;
    // that is reported against the path instead.
    if let Err(err) = dispatch(&mut stdout_terminal, &mut stderr_terminal, &config) {
        let _ = stdout_terminal.flush();
        match err.name() {
            "InvalidEditText" => {
                let Some(spliced) = types::spliced_text(&config) else { unparseable_edit(&mut stderr_terminal, &config) };
                let rendered = edit_ops::refused_rendering();
                diag_report::report_bad_edit_text(
                    &mut stderr_terminal,
                    spliced.file,
                    spliced.format,
                    spliced.kind,
                    spliced.text.map(str::as_bytes),
                    rendered.as_deref(),
                );
            }
            // The document took `0` no better than the value, so the KEY is what
            // it refused (`edit_ops::apply_value_edit`): the user typed it wrong.
            "InvalidEditKey" => {
                let Some(spliced) = types::spliced_text(&config) else { unparseable_edit(&mut stderr_terminal, &config) };
                diag_report::report_bad_edit_text(
                    &mut stderr_terminal,
                    spliced.file,
                    spliced.format,
                    types::EditTextKind::Key,
                    types::created_key(&config),
                    None,
                );
            }
            // A value argument the file's format cannot hold, from rendering it
            // (`value_arg::render`, which names these so a parse error the file
            // raises never lands here); nothing was written.
            "UnwritableNull" | "UnwritableNested" | "UnwritableKey" | "UnwritableNonFinite" => {
                diag_report::report_unwritable_value(&mut stderr_terminal, types::target_file(&config).unwrap_or("the file"), err)
            }
            // Everything an action didn't report itself stops HERE with a plain
            // line. The most common one by far is a parse failure on the target
            // file itself, which is reported where it is, below, whenever its
            // format can say where.
            _ => {
                // An action that parses its file without a report (the in-place
                // editors) lets a parse failure escape as a bare error name.
                // Parse the file again the way `check` does, and when that
                // locates the failure, the located report IS the answer — it
                // replaces the error name rather than pointing the user at a
                // second command.
                if let Some(t) = types::parse_target(&config) {
                    if report_parse_failure(&mut stderr_terminal, &t) {
                        fileio::exit(1);
                    }
                }
                diag_report::report_unhandled(&mut stderr_terminal, err, types::target_file(&config), &config.binary_name);
            }
        }
    }
    let _ = stdout_terminal.flush();
}

/// `diag_report::report_unparseable_edit` for `config`'s file and path. Every
/// action that edits has both, so the fallbacks only keep a report from
/// panicking.
fn unparseable_edit(term: &mut Terminal, config: &CliConfig) -> ! {
    diag_report::report_unparseable_edit(term, types::target_file(config).unwrap_or("the file"), types::edit_path(config).unwrap_or(&[]))
}

/// Re-parse `t.file` as `check` would and print every located parse error it
/// finds. True when there was at least one to print; false when the file
/// parses, cannot be read, or fails in a format with no located report — in
/// which case the caller's generic report is all there is to say.
fn report_parse_failure(term: &mut Terminal, t: &ParseTarget<'_>) -> bool {
    let mut source = None;
    let mut errors = None;
    let mut warnings = None;
    if parse_dispatch::check_one(t.file, t.format, None, &mut source, &mut errors, &mut warnings).is_ok() {
        return false;
    }
    let Some(errs) = errors else { return false };
    let source = source.unwrap_or_default();
    for d in &errs {
        if diag_report::print_diag(term, &source, t.file, d.offset, d.end, "error", Color::Red, &d.message, &d.short_label).is_err() {
            return true;
        }
    }
    let _ = term.flush();
    true
}

fn dispatch(stdout_terminal: &mut Terminal, stderr_terminal: &mut Terminal, config: &CliConfig) -> Result<(), Error> {
    let b = &config.binary_name;
    match &config.options {
        CliActionOptions::Help(o) => actions::run_help(stdout_terminal, stderr_terminal, b, o),
        CliActionOptions::Version => actions::run_version(stdout_terminal, VERSION, EPOCH),
        CliActionOptions::Replace(o) | CliActionOptions::Rename(o) => actions::run_edit(stderr_terminal, o),
        CliActionOptions::Set(o) => actions::run_set(stderr_terminal, o),
        CliActionOptions::Insert(o) => actions::run_insert(stderr_terminal, b, o),
        CliActionOptions::Delete(o) => actions::run_delete(stderr_terminal, o),
        CliActionOptions::Get(o) => actions::run_get(stdout_terminal, stderr_terminal, o),
        CliActionOptions::Comment(o) => actions::run_comment(stdout_terminal, stderr_terminal, o),
        CliActionOptions::Check(o) => actions::run_check(stdout_terminal, stderr_terminal, o),
        CliActionOptions::Fmt(o) => actions::run_fmt(stdout_terminal, stderr_terminal, o),
        CliActionOptions::Convert(o) => actions::run_convert(stdout_terminal, stderr_terminal, o),
        CliActionOptions::Patch(o) => actions::run_patch(stdout_terminal, stderr_terminal, o),
        CliActionOptions::Lang(o) => actions::run_lang(stdout_terminal, stderr_terminal, o),
        CliActionOptions::External(o) => actions::run_external(stdout_terminal, stderr_terminal, b, o),
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn log_lines_are_one_line_in_the_cli_voice() {
        crate::languages::init();
        // The body's trailing newline is dropped; nothing else to check
        // without capturing standard error.
        assert_eq!("No path provided.\n".trim_end_matches('\n'), "No path provided.");
    }
}
