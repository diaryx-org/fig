//! Port of src/cli/external.zig.
//!
//! The git-style subcommand handoff: a word `parse_config` has no action for
//! (`fig schema ...`) becomes a `fig-schema ...` executable looked up on PATH,
//! so a sibling tool can add a verb to `fig` without fig knowing it exists.
//! `fig-schema` is the case this was written for; nothing here knows the name.
//!
//! Two things git does that this deliberately does not:
//!
//! * git searches its own exec-path (`libexec/git-core`) before PATH. fig has
//!   no libexec directory to search — every one of its siblings installs into
//!   the same `bin` the shell already looks in, via Homebrew, npm, or cargo —
//!   so PATH is the whole lookup, and the not-found report says so.
//! * git rewrites the child's argv[0] and exports GIT_* variables to describe
//!   the repository it found. There is no equivalent state here: fig holds
//!   nothing across a command, so the child gets the environment unmodified.
//!
//! The handoff is a process REPLACEMENT wherever the OS has one. See `run`.

use fig_core::{err, Error, Result};

use crate::fileio::{self, Color, Terminal};
use crate::help::Help;
use crate::types::ExternalOptions;

/// Hand `opts.argv` to `fig-<name>`. Never returns normally: this process
/// either becomes the other tool, exits with the status it exited with, or
/// exits 2 having reported why the handoff never happened.
pub fn run(stdout_term: &mut Terminal, stderr_term: &mut Terminal, binary_name: &str, opts: &ExternalOptions) -> Result<()> {
    // A word that could never name an executable (`fig config.toml`) never
    // reaches PATH — `parse_config` marks it with a `None` `program`. It
    // arrives here anyway so that "no such action" is worded in one place.
    if opts.program.is_none() {
        report(stderr_term, binary_name, opts, None);
    }

    // Both standard-stream writers may hold bytes; the child inherits the
    // descriptors, so anything still sitting in a buffer here would be lost
    // outright on the replace path, where this image stops existing.
    stdout_term.flush()?;
    stderr_term.flush()?;

    #[cfg(unix)]
    {
        // Replace the image rather than spawn a child. `fig schema` is then
        // the same process the shell started, which is what makes the handoff
        // invisible: one PID, the child's exit status is this command's exit
        // status with nothing to translate, and signals, job control, and the
        // terminal's foreground process group all address the running tool
        // directly instead of a fig that is only waiting on it.
        //
        // `exec` returns only on failure.
        use std::os::unix::process::CommandExt;
        let e = std::process::Command::new(&opts.argv[0]).args(&opts.argv[1..]).exec();
        report(stderr_term, binary_name, opts, Some(fileio::io_error(&e)));
    }
    #[cfg(not(unix))]
    {
        // Windows, which has no exec: spawn and forward the status by hand.
        let status = match std::process::Command::new(&opts.argv[0]).args(&opts.argv[1..]).status() {
            Ok(s) => s,
            Err(e) => report(stderr_term, binary_name, opts, Some(fileio::io_error(&e))),
        };
        fileio::exit(status.code().unwrap_or(1));
    }
}

/// Report a handoff that never happened, and exit 2 — the same status every
/// other unusable command line leaves. `err` is `None` when there was nothing
/// to attempt (`opts.program` `None`); `FileNotFound` is the attempt that found
/// nothing, which is the same answer to the user and by far the common one.
fn report(term: &mut Terminal, binary_name: &str, opts: &ExternalOptions, err: Option<Error>) -> ! {
    let _ = report_impl(term, binary_name, opts, err);
    let _ = term.flush();
    fileio::exit(2);
}

fn report_impl(term: &mut Terminal, binary_name: &str, opts: &ExternalOptions, err: Option<Error>) -> Result<()> {
    // The two shapes of "fig has no such action". Both end in the action
    // list, which is the useful half of the answer for what this nearly
    // always is: a misspelling of a verb fig does have.
    //
    // "a fig command", not "a `{binary_name}` command": the lookup is spelled
    // `fig-` whatever argv[0] was, so naming the path the user happened to
    // invoke would describe a rule that isn't the one being applied.
    if err.is_none() || err == Some(err!(FileNotFound)) {
        if let Some(program) = &opts.program {
            write_error(term, &format!("`{}` is not a fig command, and no `{}` was found on your PATH.\n", opts.name, program))?;
            // A verb fig had and no longer does — said only once the PATH
            // lookup found nothing, so a real `fig-edit` still runs.
            if opts.name == "edit" || opts.name == "e" {
                write_note(term, "fig 5 split `edit` in two: `fig replace <file> <path> <value>` replaces a value, `fig rename <file> <path> <name>` renames a key.\n")?;
            } else {
                write_note(term, "install the tool that provides it, or pick one of the actions below.\n")?;
            }
        } else {
            write_error(term, &format!("`{}` is not a fig command.\n", opts.name))?;
        }
        term.write_str("\n")?;
        return Help::general(term, binary_name);
    }

    let program = opts.program.as_deref().unwrap_or("");
    let e = err.unwrap();
    match e.name() {
        "AccessDenied" | "PermissionDenied" => {
            write_error(term, &format!("`{program}` was found on your PATH but is not executable.\n"))?;
        }
        // A build that cannot run another program at all. The handoff is
        // the only part of the CLI that needs one, so it is the only action
        // such a build cannot perform.
        "OperationUnsupported" => {
            write_error(term, &format!("this build of fig cannot run other programs, so `{}` has nowhere to go.\n", opts.name))?;
        }
        _ => {
            write_error(term, &format!("could not run `{program}`: {}\n", e.name()))?;
        }
    }
    Ok(())
}

fn write_error(term: &mut Terminal, text: &str) -> Result<()> {
    term.set_color(Color::Red)?;
    term.write_str("error")?;
    term.set_color(Color::Reset)?;
    term.write_str(": ")?;
    term.write_str(text)
}

fn write_note(term: &mut Terminal, text: &str) -> Result<()> {
    term.set_color(Color::Blue)?;
    term.write_str("note")?;
    term.set_color(Color::Reset)?;
    term.write_str(": ")?;
    term.write_str(text)
}
