//! The git-style subcommand handoff: a word `parseConfig` has no action for
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

const std = @import("std");
const Io = std.Io;
const process = std.process;

const types = @import("types.zig");
const help = @import("help.zig");

const Help = help.Help;

/// Hand `opts.argv` to `fig-<name>`. Never returns normally: this process
/// either becomes the other tool, exits with the status it exited with, or
/// exits 2 having reported why the handoff never happened.
pub fn run(
    io: Io,
    stdout_term: *Io.Terminal,
    stderr_term: *Io.Terminal,
    binary_name: []const u8,
    opts: types.ExternalOptions,
) !void {
    // A word that could never name an executable (`fig config.toml`) never
    // reaches PATH — `parseConfig` marks it with a null `program`. It arrives
    // here anyway so that "no such action" is worded in one place.
    if (opts.program == null) report(stderr_term, binary_name, opts, null);

    // Both standard-stream writers are buffered, and (see `fileio.stdioWriter`)
    // stream rather than write positionally, so their bytes land at the shared
    // offset whenever they are flushed. The child inherits those descriptors:
    // anything still sitting in a buffer here would be written after the
    // child's output on the spawn path, and lost outright on the replace path,
    // where this image stops existing. Nothing has been written yet in
    // practice; flushing is what makes that not matter.
    try stdout_term.writer.flush();
    try stderr_term.writer.flush();

    if (comptime process.can_replace) {
        // Replace the image rather than spawn a child. `fig schema` is then
        // the same process the shell started, which is what makes the handoff
        // invisible: one PID, the child's exit status is this command's exit
        // status with nothing to translate, and signals, job control, and the
        // terminal's foreground process group all address the running tool
        // directly instead of a fig that is only waiting on it.
        //
        // `replace` returns only on failure — its return type is the error set
        // itself, not an error union.
        report(stderr_term, binary_name, opts, process.replace(io, .{ .argv = opts.argv }));
    } else if (comptime process.can_spawn) {
        // Windows, which has no exec: spawn and forward the status by hand.
        var child = process.spawn(io, .{ .argv = opts.argv }) catch |err|
            report(stderr_term, binary_name, opts, err);
        switch (try child.wait(io)) {
            .exited => |code| process.exit(code),
            // A process that died by a signal has no exit status of its own.
            // Synthesize the one every shell reports for it, so `fig schema`
            // and `fig-schema` are still indistinguishable to a caller
            // checking `$?`.
            .signal, .stopped => |sig| process.exit(128 +| @as(u8, @truncate(@intFromEnum(sig)))),
            .unknown => process.exit(1),
        }
    } else {
        // WASI: no exec, no fork. Nothing to hand off to, and no PATH to
        // report having searched.
        report(stderr_term, binary_name, opts, error.OperationUnsupported);
    }
}

/// Report a handoff that never happened, and exit 2 — the same status every
/// other unusable command line leaves. `err` is null when there was nothing to
/// attempt (`opts.program` null); `FileNotFound` is the attempt that found
/// nothing, which is the same answer to the user and by far the common one.
fn report(
    term: *Io.Terminal,
    binary_name: []const u8,
    opts: types.ExternalOptions,
    err: ?anyerror,
) noreturn {
    reportImpl(term, binary_name, opts, err) catch {};
    term.writer.flush() catch {};
    process.exit(2);
}

fn reportImpl(
    term: *Io.Terminal,
    binary_name: []const u8,
    opts: types.ExternalOptions,
    err: ?anyerror,
) !void {
    // The two shapes of "fig has no such action". Both end in the action
    // list, which is the useful half of the answer for what this nearly
    // always is: a misspelling of a verb fig does have.
    //
    // "a fig command", not "a `{binary_name}` command": the lookup is spelled
    // `fig-` whatever argv[0] was, so naming the path the user happened to
    // invoke would describe a rule that isn't the one being applied.
    if (err == null or err.? == error.FileNotFound) {
        if (opts.program) |program| {
            try writeError(term, "`{s}` is not a fig command, and no `{s}` was found on your PATH.\n", .{ opts.name, program });
            // The rule itself is stated in the action list below (see
            // `Help.general`); this says only what to do about it.
            try writeNote(term, "install the tool that provides it, or pick one of the actions below.\n");
        } else {
            try writeError(term, "`{s}` is not a fig command.\n", .{opts.name});
        }
        try term.writer.writeAll("\n");
        return Help.general(term, binary_name);
    }

    const program = opts.program.?;
    switch (err.?) {
        error.AccessDenied, error.PermissionDenied => {
            try writeError(term, "`{s}` was found on your PATH but is not executable.\n", .{program});
        },
        // WASI, where there is no way to run another program at all. The
        // handoff is the only part of the CLI that needs one, so it is the
        // only action this build cannot perform.
        error.OperationUnsupported => {
            try writeError(term, "this build of fig cannot run other programs, so `{s}` has nowhere to go.\n", .{opts.name});
        },
        else => {
            try writeError(term, "could not run `{s}`: {s}\n", .{ program, @errorName(err.?) });
        },
    }
}

fn writeError(term: *Io.Terminal, comptime format: []const u8, args: anytype) !void {
    try term.setColor(.red);
    try term.writer.writeAll("error");
    try term.setColor(.reset);
    try term.writer.writeAll(": ");
    try term.writer.print(format, args);
}

fn writeNote(term: *Io.Terminal, text: []const u8) !void {
    try term.setColor(.blue);
    try term.writer.writeAll("note");
    try term.setColor(.reset);
    try term.writer.writeAll(": ");
    try term.writer.writeAll(text);
}
