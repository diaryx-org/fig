//! Low-level file/stdin plumbing shared by every CLI action: opening the
//! target (or `-` for stdin), reading it whole, and the create/seed/rollback
//! dance `set` uses when its target file doesn't exist yet.
const std = @import("std");
const Io = std.Io;

/// Currently, `fig` CLI only supports up to 10MB files.
pub const max_size = Io.Limit.limited(10 * 1024 * 1024);

pub fn getInput(io: Io, file_path: ?[]const u8, mode: std.Io.Dir.OpenFileOptions.Mode) !Io.File {
    const log = std.log.scoped(.getInput);
    // Get input file descriptor
    if (file_path) |fp| {
        if (std.mem.eql(u8, fp, "-")) {
            return Io.File.stdin();
        } else {
            // Get current working directory
            var cwd_buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
            const cwd_path = try std.process.currentPath(io, &cwd_buf);
            const cwd = cwd_buf[0..cwd_path];
            log.debug("opening {s} in {s}", .{ fp, cwd });

            // Open directory (scope to files in this directory)
            const dir = try std.Io.Dir.cwd().openDir(io, cwd, .{});
            defer dir.close(io);

            // Open file, handle if it doesn't exist
            return dir.openFile(io, fp, .{ .mode = mode });
        }
    } else {
        log.err("No file provided.", .{});
        return error.MissingArgument;
    }
}

pub fn readAll(allocator: std.mem.Allocator, io: Io, file: Io.File) ![]u8 {
    var read_buffer: [4096]u8 = undefined;
    // Standard input takes the streaming path for the reason `stdioReader`
    // gives; a file this process opened is at offset 0 by construction, so it
    // keeps the positional read.
    var file_reader = if (file.handle == Io.File.stdin().handle)
        stdioReader(file, io, &read_buffer)
    else
        file.reader(io, &read_buffer);
    return file_reader.interface.allocRemaining(allocator, max_size);
}

/// Create `file_path` for read+write and seed it with `seed` — the `set`
/// action's "upsert into nothing" path (a `touch` folded into the existing
/// upsert verb). Most editors can't parse a truly empty buffer, so the file is
/// primed with a minimal valid empty document for its format (`{}` for JSON,
/// `.{}` for ZON, nothing for YAML/TOML — see `emptyDocSeed` in `edit_ops.zig`);
/// the subsequent `set` then lands the first key into a parseable document,
/// exactly as an absent embed block is seeded before its first key. Writing is
/// positional and leaves the read cursor at 0, so `applyToFile`'s `readAll`
/// reads the seed back.
pub fn createSeededFile(io: Io, file_path: []const u8, seed: []const u8) !Io.File {
    var cwd_buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const cwd_path = try std.process.currentPath(io, &cwd_buf);
    const dir = try std.Io.Dir.cwd().openDir(io, cwd_buf[0..cwd_path], .{});
    defer dir.close(io);
    const file = try dir.createFile(io, file_path, .{ .read = true });
    if (seed.len > 0) try file.writePositionalAll(io, seed, 0);
    return file;
}

/// Best-effort unlink of a file `set` just created, used to roll back a
/// from-scratch create when the edit that followed it failed — so a failed
/// `set` never leaves a bare seed document (`{}`, `.{}`, …) littering the tree.
/// Silent on failure: this is cleanup on an already-failing path, and the edit
/// error is what the user needs to see.
pub fn deleteCreatedFile(io: Io, file_path: []const u8) void {
    var cwd_buf: [std.Io.Dir.max_path_bytes]u8 = undefined;
    const cwd_path = std.process.currentPath(io, &cwd_buf) catch return;
    const dir = std.Io.Dir.cwd().openDir(io, cwd_buf[0..cwd_path], .{}) catch return;
    defer dir.close(io);
    dir.deleteFile(io, file_path) catch {};
}

/// A writer over one of this process's standard streams.
///
/// stdin/stdout/stderr are inherited file DESCRIPTIONS: the seek offset
/// belongs to whoever opened the redirection — the shell — and is shared with
/// every sibling process on the same stream. `File.writer` defaults to
/// POSITIONAL writes, which start at byte 0 and ignore that offset entirely.
/// On a pipe or a tty that is harmless (there is no offset to respect, pwrite
/// fails, and the writer falls back to streaming), which is exactly why it
/// survives interactive use and every pipeline in the test suite. Redirect
/// stdout to a regular FILE, though, and each invocation writes over the front
/// of whatever is already there:
///
///     $ bash -c 'echo AAAAAAAAAA; fig version; echo BBBBBBBBBB' > x
///     $ cat x
///     fig 3.6.0 (BBBBBBBBBB
///     "Sierra")
///
/// `fig version` landed at byte 0, over the `echo` before it, and the `echo`
/// after it landed at the offset the shell had reached — inside fig's output.
/// Any `fig ... >> log`, or any redirected block that runs fig more than once,
/// hits this.
///
/// Streaming mode uses plain `write`, which respects and advances the shared
/// offset. Every writer over a standard stream is built through here.
///
/// A file this process OPENED itself is the opposite case and keeps the
/// positional path: its offset is 0 by construction, nobody else holds the
/// description, and positional is the more threadsafe of the two.
pub fn stdioWriter(file: Io.File, io: Io, buffer: []u8) Io.File.Writer {
    return file.writerStreaming(io, buffer);
}

/// The read counterpart of `stdioWriter`, for the same reason: a positional
/// read of standard input starts at byte 0 however much of the stream a
/// process sharing the description has already consumed.
pub fn stdioReader(file: Io.File, io: Io, buffer: []u8) Io.File.Reader {
    return file.readerStreaming(io, buffer);
}

test "a standard-stream writer appends at the shared offset instead of rewriting from zero" {
    const t = std.testing;
    const io = t.io;

    var tmp = t.tmpDir(.{});
    defer tmp.cleanup();

    // Stand in for a redirected stdout: one handle, written by two parties.
    // The first advances the description's offset the way the shell's `echo`
    // does; the second is fig.
    const file = try tmp.dir.createFile(io, "out", .{ .read = true });
    defer file.close(io);
    try file.writeStreamingAll(io, "AAAA\n");

    var buf: [64]u8 = undefined;
    var w = stdioWriter(file, io, &buf);
    try w.interface.writeAll("BBBB\n");
    try w.interface.flush();

    // With a positional writer this reads "BBBB\n" alone: fig's bytes would
    // have gone to offset 0, on top of what was already there.
    const back = try tmp.dir.readFileAlloc(io, "out", t.allocator, max_size);
    defer t.allocator.free(back);
    try t.expectEqualStrings("AAAA\nBBBB\n", back);
}
