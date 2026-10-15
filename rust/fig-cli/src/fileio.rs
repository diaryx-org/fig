//! Port of src/cli/fileio.zig.
//!
//! Low-level file/stdin plumbing shared by every CLI action: opening the
//! target (or `-` for stdin), reading it whole, and the create/seed/rollback
//! dance `set` uses when its target file doesn't exist yet.
//!
//! Also the standard streams as the CLI writes them: Zig's `std.Io.Terminal`
//! (a writer plus a colour mode), which the Zig CLI builds over
//! `stdioWriter`, is `Terminal` here.

use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};

use fig_core::{err, Error, Result};

/// Currently, `fig` CLI only supports up to 10MB files.
pub const MAX_SIZE: usize = 10 * 1024 * 1024;

/// `std.Io.Dir.OpenFileOptions.Mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    ReadOnly,
    ReadWrite,
}

/// An open input: standard input, or a file this process opened.
pub enum Input {
    Stdin,
    File(File),
}

/// An `std::io::Error` as the Zig error name its `Io` call would have
/// returned.
pub fn io_error(e: &std::io::Error) -> Error {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => err!(FileNotFound),
        K::PermissionDenied => err!(AccessDenied),
        K::IsADirectory => err!(IsDir),
        K::NotADirectory => err!(NotDir),
        K::AlreadyExists => err!(PathAlreadyExists),
        K::InvalidInput => err!(BadPathName),
        K::BrokenPipe => err!(WriteFailed),
        _ => match e.raw_os_error() {
            Some(21) => err!(IsDir),
            Some(36) => err!(NameTooLong),
            _ => err!(Unexpected),
        },
    }
}

pub fn get_input(file_path: Option<&str>, mode: OpenMode) -> Result<Input> {
    // Get input file descriptor
    match file_path {
        Some(fp) => {
            if fp == "-" {
                Ok(Input::Stdin)
            } else {
                // Open file, handle if it doesn't exist
                let mut o = OpenOptions::new();
                o.read(true);
                if mode == OpenMode::ReadWrite {
                    o.write(true);
                }
                // A directory opens read-only (and fails its read, as
                // `ReadFailed`); for writing, the open is `IsDir`.
                let f = o.open(fp).map_err(|e| io_error(&e))?;
                Ok(Input::File(f))
            }
        }
        None => {
            crate::log_err!("No file provided.");
            Err(err!(MissingArgument))
        }
    }
}

pub fn read_all(file: &mut Input) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // Standard input takes the streaming path for the reason `stdio_reader`
    // gives; a file this process opened is at offset 0 by construction, so it
    // keeps the positional read.
    let r = match file {
        Input::Stdin => std::io::stdin().lock().take(MAX_SIZE as u64 + 1).read_to_end(&mut out),
        Input::File(f) => {
            f.seek(SeekFrom::Start(0)).map_err(|e| io_error(&e))?;
            f.take(MAX_SIZE as u64 + 1).read_to_end(&mut out)
        }
    };
    // Zig's reader reports every failed read as `ReadFailed`.
    r.map_err(|_| err!(ReadFailed))?;
    if out.len() > MAX_SIZE {
        return Err(err!(StreamTooLong));
    }
    Ok(out)
}

impl Input {
    /// `writePositionalAll(bytes, 0)` then `setLength(bytes.len)`: replace the
    /// file's whole content in place.
    pub fn rewrite(&mut self, bytes: &[u8]) -> Result<()> {
        match self {
            Input::Stdin => Err(err!(NotOpenForWriting)),
            Input::File(f) => {
                f.seek(SeekFrom::Start(0)).map_err(|e| io_error(&e))?;
                f.write_all(bytes).map_err(|e| io_error(&e))?;
                f.set_len(bytes.len() as u64).map_err(|e| io_error(&e))?;
                Ok(())
            }
        }
    }
}

/// Create `file_path` for read+write and seed it with `seed` — the `set`
/// action's "upsert into nothing" path (a `touch` folded into the existing
/// upsert verb). Most editors can't parse a truly empty buffer, so the file is
/// primed with a minimal valid empty document for its format (`{}` for JSON,
/// `.{}` for ZON, nothing for YAML/TOML — see `empty_doc_seed` in `edit_ops.rs`);
/// the subsequent `set` then lands the first key into a parseable document.
pub fn create_seeded_file(file_path: &str, seed: &[u8]) -> Result<Input> {
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(file_path)
        .map_err(|e| io_error(&e))?;
    if !seed.is_empty() {
        f.write_all(seed).map_err(|e| io_error(&e))?;
    }
    Ok(Input::File(f))
}

/// Best-effort unlink of a file `set` just created, used to roll back a
/// from-scratch create when the edit that followed it failed — so a failed
/// `set` never leaves a bare seed document (`{}`, `.{}`, …) littering the tree.
/// Silent on failure: this is cleanup on an already-failing path, and the edit
/// error is what the user needs to see.
pub fn delete_created_file(file_path: &str) {
    let _ = std::fs::remove_file(file_path);
}

// ── the standard streams (Zig's `std.Io.Terminal` over `stdioWriter`) ──────

/// A colour `Terminal::set_color` writes (the members of Zig's
/// `Io.Terminal.Color` the CLI uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Red,
    Green,
    Yellow,
    Blue,
    Reset,
}

/// `Io.Terminal.Mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    NoColor,
    EscapeCodes,
}

impl Mode {
    /// Detect suitable TTY configuration for the stream `is_tty` says
    /// about: `NO_COLOR` off, `CLICOLOR_FORCE` on, else whether it is a
    /// terminal (`Io.Terminal.Mode.detect`).
    pub fn detect(is_tty: bool, no_color: bool, clicolor_force: bool) -> Mode {
        let force_color: Option<bool> = if no_color {
            Some(false)
        } else if clicolor_force {
            Some(true)
        } else {
            None
        };
        if force_color == Some(false) {
            return Mode::NoColor;
        }
        if is_tty {
            return Mode::EscapeCodes;
        }
        if force_color == Some(true) { Mode::EscapeCodes } else { Mode::NoColor }
    }
}

/// Where a `Terminal`'s bytes go.
pub enum Sink {
    /// Standard output, buffered until `flush`.
    Stdout,
    /// Standard error, written as it comes: the CLI's log lines go there
    /// too, so nothing of it may sit in a buffer of its own.
    Stderr,
    /// Kept, for a test to read.
    #[allow(dead_code)]
    Buffer,
    /// Thrown away.
    #[allow(dead_code)]
    Discard,
}

/// A writer that might support terminal escape codes (`std.Io.Terminal`).
///
/// stdin/stdout/stderr are inherited file DESCRIPTIONS whose seek offset
/// belongs to whoever opened the redirection; both streams here write with
/// plain `write`, which respects and advances that shared offset, never
/// positionally from byte 0 (Zig's `stdioWriter`, which explains why).
pub struct Terminal {
    pub mode: Mode,
    sink: Sink,
    buf: Vec<u8>,
}

impl Terminal {
    pub fn new(sink: Sink, mode: Mode) -> Terminal {
        Terminal { mode, sink, buf: Vec::new() }
    }

    /// Standard output, its colour mode detected.
    pub fn stdout(no_color: bool, clicolor_force: bool) -> Terminal {
        let mode = Mode::detect(std::io::stdout().is_terminal(), no_color, clicolor_force);
        Terminal::new(Sink::Stdout, mode)
    }

    /// Standard error, its colour mode detected.
    pub fn stderr(no_color: bool, clicolor_force: bool) -> Terminal {
        let mode = Mode::detect(std::io::stderr().is_terminal(), no_color, clicolor_force);
        Terminal::new(Sink::Stderr, mode)
    }

    /// What a `Buffer` terminal holds.
    #[allow(dead_code)]
    pub fn written(&self) -> &[u8] {
        &self.buf
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        match self.sink {
            Sink::Stderr => {
                let mut e = std::io::stderr().lock();
                e.write_all(bytes).map_err(|_| err!(WriteFailed))
            }
            Sink::Discard => Ok(()),
            Sink::Stdout | Sink::Buffer => {
                self.buf.extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    pub fn write_str(&mut self, s: &str) -> Result<()> {
        self.write_all(s.as_bytes())
    }

    pub fn write_byte(&mut self, b: u8) -> Result<()> {
        self.write_all(&[b])
    }

    /// `splatByteAll`: `n` copies of `b`.
    pub fn splat_byte_all(&mut self, b: u8, n: usize) -> Result<()> {
        self.write_all(&vec![b; n])
    }

    /// `print`: formatted text.
    pub fn print(&mut self, args: std::fmt::Arguments<'_>) -> Result<()> {
        match args.as_str() {
            Some(s) => self.write_all(s.as_bytes()),
            None => self.write_all(std::fmt::format(args).as_bytes()),
        }
    }

    pub fn set_color(&mut self, color: Color) -> Result<()> {
        match self.mode {
            Mode::NoColor => Ok(()),
            Mode::EscapeCodes => {
                let s: &[u8] = match color {
                    Color::Red => b"\x1b[31m",
                    Color::Green => b"\x1b[32m",
                    Color::Yellow => b"\x1b[33m",
                    Color::Blue => b"\x1b[34m",
                    Color::Reset => b"\x1b[0m",
                };
                self.write_all(s)
            }
        }
    }

    pub fn flush(&mut self) -> Result<()> {
        if let Sink::Stdout = self.sink {
            if !self.buf.is_empty() {
                let mut o = std::io::stdout().lock();
                let r = o.write_all(&self.buf).and_then(|_| o.flush());
                self.buf.clear();
                r.map_err(|_| err!(WriteFailed))?;
            }
        }
        Ok(())
    }
}

/// `std.process.exit`, with standard output's buffer written first — the
/// Zig CLI flushes before every exit it takes.
pub fn exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}
