//! A format as a description: combinators over a `Scanner` that write the
//! node table as they match, so the spans, the comment binding and the
//! duplicate policy follow from the grammar.
//!
//! The JavaScript kit's `@diaryx/fig/kit/grammar` and Rust's
//! `fig::kit::grammar`, in Zig: the same rules, the same messages, the same
//! comment binding. A rule is a type, built at compile time, with
//!
//!     pub const Out = T;
//!     pub fn match(cx: *Ctx) Error!?T
//!
//! answering a `T` when it matches, null when it does not (the cursor where
//! it was, so a `choice` tries the next), or `error.Refused` with the
//! message and offset in `cx.failure`. The primitives answer spans; the
//! rules that make nodes (`scalar`, `key`, `bare`, `quoted`, `entry`, `map`,
//! `sequence`) write rows through a `RowWriter` and answer their ids;
//! `Document` reads a whole input into a table, and `Parser` makes it a
//! Zig language's parser. A rule allocates nothing a node does not.
//!
//! Comments are bound as trivia is skipped: a comment on the same line as
//! the last node completed is that node's trailing comment; any other
//! waits, and becomes the leading comment of the next entry (on its key, by
//! default) or, when a container closes with it still waiting, a dangling
//! comment of that container. `Binding` takes the refinements some formats
//! add.
//!
//! Where the JavaScript grammar matches a regular expression, this one
//! takes a byte predicate (`bytesWhile`, `ident`).

const std = @import("std");
const Allocator = std.mem.Allocator;
const Span = @import("span.zig");
const Scanner = @import("scanner.zig");
const RowWriter = @import("row_writer.zig");
const ChildIndex = @import("child_index.zig");
const flat_map = @import("flat_map.zig");
const table = @import("table.zig");
const AST = @import("ast/ast.zig");
const Document_ = @import("document.zig");
const Node = AST.Node;

pub const Error = error{Refused} || Allocator.Error;

/// Why an input was refused, and where.
pub const Failure = struct { message: []const u8, at: usize };

/// How comments bind beyond the default.
pub const Binding = struct {
    /// A comment trails only when it is the last thing on its line.
    ends_line: bool = false,
    /// A node takes one trailing comment; a second waits like any other.
    one: bool = false,
    /// A container that spans lines takes the comment on its closing line
    /// as dangling rather than trailing.
    closing_line: bool = false,
};

/// A comment a `comment` rule matched.
pub const Comment = struct { span: Span, text: []const u8, style: AST.Comment.Style };

/// Which of an entry's nodes a comment is bound to.
pub const Target = enum { key, value, entry };

/// What every rule runs over: the scanner, the rows written so far and an
/// index of each container's last child and each mapping's entries, the
/// comments waiting to be bound, and the refusal once there is one.
pub const Ctx = struct {
    allocator: Allocator,
    sc: Scanner,
    w: RowWriter,
    index: ChildIndex = .{},
    pending: std.ArrayList(Comment) = .empty,
    /// The node a comment on its line trails.
    last: ?Node.Id = null,
    binding: Binding = .{},
    failure: ?Failure = null,
    message_buf: [256]u8 = undefined,

    /// A context over `input`, writing to `t`.
    pub fn init(allocator: Allocator, t: *table.Table, input: []const u8, binding: Binding) Ctx {
        return .{ .allocator = allocator, .sc = .init(input), .w = .init(t), .binding = binding };
    }

    pub fn deinit(self: *Ctx) void {
        self.index.deinit(self.allocator);
        self.pending.deinit(self.allocator);
    }

    /// Refuse the input with `message` at `at`.
    pub fn fail(self: *Ctx, message: []const u8, at: usize) Error {
        self.failure = .{ .message = message, .at = at };
        return error.Refused;
    }

    /// Refuse the input with `message` at the cursor.
    pub fn failHere(self: *Ctx, message: []const u8) Error {
        return self.fail(message, self.sc.pos);
    }

    /// Refuse the input with a message formatted into the context's own
    /// buffer, at `at`.
    pub fn failFmt(self: *Ctx, at: usize, comptime fmt: []const u8, args: anytype) Error {
        const m = std.fmt.bufPrint(&self.message_buf, fmt, args) catch self.message_buf[0..];
        return self.fail(m, at);
    }

    /// Row `id`'s span.
    pub fn span(self: *Ctx, id: Node.Id) Span {
        return self.w.spanOf(id);
    }

    /// Bind comment `c`, met in trivia: trailing on the last node when it
    /// is on that node's line, else waiting.
    pub fn comment(self: *Ctx, c: Comment) Error!void {
        if (self.last) |last| {
            const s = self.span(last);
            if (self.sc.sameLine(s.end, c.span.start) and (!self.binding.ends_line or self.restIsBlank(c.span.end))) {
                const k = self.w.t.rows.items[last].kind;
                const container = k == table.Kind.mapping or k == table.Kind.sequence;
                const slot = if (self.binding.closing_line and container and !self.sc.sameLine(s.start, s.end))
                    table.comment_dangling
                else
                    table.comment_trailing;
                try self.w.comment(last, slot, .{ .text = c.text, .style = c.style });
                if (self.binding.one) self.last = null;
                return;
            }
        }
        try self.pending.append(self.allocator, c);
        self.last = null;
    }

    fn restIsBlank(self: *Ctx, at: usize) bool {
        const b = self.sc.src;
        var i = at;
        while (i < b.len and Scanner.isHs(b[i])) i += 1;
        return i >= b.len or b[i] == '\n' or b[i] == '\r';
    }

    /// Give every waiting comment to row `node`, in `slot`.
    pub fn flush(self: *Ctx, node: Node.Id, slot: u8) Error!void {
        for (self.pending.items) |c| try self.w.comment(node, slot, .{ .text = c.text, .style = c.style });
        self.pending.clearRetainingCapacity();
    }

    /// Put keyvalue `kv` in mapping `map` by `policy`, as `flat_map`
    /// does for a keyvalue already written: on a repeat that the policy
    /// does not keep, `kv` and its key are left unlinked for `finish` to
    /// drop.
    pub fn putEntry(self: *Ctx, into: Node.Id, kv: Node.Id, policy: flat_map.DuplicatePolicy) Error!void {
        const e = self.w.kindOf(kv).keyvalue;
        const key_kind = self.w.kindOf(e.key);
        const name: ?[]const u8 = switch (key_kind) {
            .string => |s| s,
            else => null,
        };
        const repeat = if (name) |n| self.index.get(self.w, into, n) else null;
        if (repeat) |existing| switch (policy) {
            .overwrite => {
                const old_key = self.w.kindOf(existing).keyvalue.key;
                try self.w.set(existing, .{ .keyvalue = .{ .key = old_key, .value = e.value } });
                return;
            },
            .first_value => return,
            .err => return self.failFmt(self.span(e.key).start, "duplicate key `{s}`", .{name.?}),
            .keep => {},
        };
        try self.index.append(self.allocator, self.w, into, kv);
        if (repeat == null and name != null) try self.index.put(self.allocator, self.w, kv);
    }
};

// ── primitives ──────────────────────────────────────────────────────────────

/// The literal `s`.
pub fn lit(comptime s: []const u8) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            return cx.sc.lit(s);
        }
    };
}

/// Bytes while `f` holds of them, possibly none.
pub fn bytesWhile(comptime f: fn (u8) bool) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            return cx.sc.takeWhile(f);
        }
    };
}

/// Bytes while `f` holds of them, at least one.
pub fn bytesWhile1(comptime f: fn (u8) bool) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            return cx.sc.takeWhile1(f);
        }
    };
}

/// One byte of which `first` holds, then bytes while `rest` does.
pub fn ident(comptime first: fn (u8) bool, comptime rest: fn (u8) bool) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            const s = cx.sc.pos;
            const b = cx.sc.byte(0) orelse return null;
            if (!first(b)) return null;
            cx.sc.advance(1);
            _ = cx.sc.takeWhile(rest);
            return Span.init(s, cx.sc.pos);
        }
    };
}

/// Matches, empty, where `f` holds of the scanner.
pub fn when(comptime f: fn (*const Scanner) bool) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            return if (f(&cx.sc)) Span.init(cx.sc.pos, cx.sc.pos) else null;
        }
    };
}

fn atEof(sc: *const Scanner) bool {
    return sc.eof();
}

fn isWs(c: u8) bool {
    return c == ' ' or c == '\t' or c == '\r' or c == '\n';
}

/// The end of the input.
pub const eof = when(atEof);
/// A line ending: `\n` or `\r\n`.
pub const eol = choice(.{ lit("\n"), lit("\r\n") });
/// Spaces and tabs, possibly none; at least one; all whitespace; at least
/// one whitespace.
pub const hs = bytesWhile(Scanner.isHs);
pub const hs1 = bytesWhile1(Scanner.isHs);
pub const ws = bytesWhile(isWs);
pub const ws1 = bytesWhile1(isWs);

// ── combinators ─────────────────────────────────────────────────────────────

/// Each rule in turn; the whole span.
pub fn seq(comptime rules: anytype) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            const s = cx.sc.pos;
            inline for (rules) |R| {
                if (try R.match(cx) == null) {
                    cx.sc.pos = s;
                    return null;
                }
            }
            return Span.init(s, cx.sc.pos);
        }
    };
}

/// The first rule that matches. Every rule answers the first's `Out`.
pub fn choice(comptime rules: anytype) type {
    return struct {
        pub const Out = rules[0].Out;
        pub fn match(cx: *Ctx) Error!?Out {
            const s = cx.sc.pos;
            inline for (rules) |R| {
                if (try R.match(cx)) |v| return v;
                cx.sc.pos = s;
            }
            return null;
        }
    };
}

/// `R`, or null where it does not match; always matches.
pub fn opt(comptime R: type) type {
    return struct {
        pub const Out = ?R.Out;
        pub fn match(cx: *Ctx) Error!??R.Out {
            const s = cx.sc.pos;
            if (try R.match(cx)) |v| return @as(?R.Out, v);
            cx.sc.pos = s;
            return @as(?R.Out, null);
        }
    };
}

/// `R` as many times as it matches and moves; the span of them all.
pub fn many(comptime R: type) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            const s = cx.sc.pos;
            while (true) {
                const p = cx.sc.pos;
                if (try R.match(cx) == null or cx.sc.pos == p) {
                    cx.sc.pos = p;
                    break;
                }
            }
            return Span.init(s, cx.sc.pos);
        }
    };
}

/// `R` at least once.
pub fn many1(comptime R: type) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            const v = (try many(R).match(cx)).?;
            return if (v.len() == 0) null else v;
        }
    };
}

/// Whether `R` matches here, without moving.
pub fn ahead(comptime R: type) type {
    return struct {
        pub const Out = R.Out;
        pub fn match(cx: *Ctx) Error!?Out {
            const s = cx.sc.pos;
            const v = try R.match(cx);
            cx.sc.pos = s;
            return v;
        }
    };
}

/// Matches, empty, only where `R` does not.
pub fn not(comptime R: type) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            const s = cx.sc.pos;
            const v = try R.match(cx);
            cx.sc.pos = s;
            return if (v == null) Span.init(s, s) else null;
        }
    };
}

/// `R`, or refuse the input with `message` at the cursor.
pub fn expect(comptime R: type, comptime message: []const u8) type {
    return struct {
        pub const Out = R.Out;
        pub fn match(cx: *Ctx) Error!?Out {
            return try R.match(cx) orelse cx.failHere(message);
        }
    };
}

/// Refuse the input with `message` at the cursor.
pub fn fail(comptime message: []const u8) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            return cx.failHere(message);
        }
    };
}

/// Refuse the input with `message` where `R` matches; otherwise empty.
pub fn failIf(comptime R: type, comptime message: []const u8) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            const s = cx.sc.pos;
            const v = try R.match(cx);
            cx.sc.pos = s;
            if (v != null) return cx.failHere(message);
            return Span.init(s, s);
        }
    };
}

/// `R`, its answer passed through `f`.
pub fn apply(comptime R: type, comptime U: type, comptime f: fn (*Ctx, R.Out) Error!U) type {
    return struct {
        pub const Out = U;
        pub fn match(cx: *Ctx) Error!?U {
            const v = try R.match(cx) orelse return null;
            return try f(cx, v);
        }
    };
}

// ── nodes ───────────────────────────────────────────────────────────────────

/// A scalar row's kind, before its text gives it one.
pub const ScalarKind = enum { null_, boolean, int, float, string };

/// How a scalar rule makes its row.
pub const ScalarOpts = struct {
    /// The row's text from what was matched; the matched text itself
    /// without it. What it answers is the caller's to keep alive until the
    /// row is written, which copies it.
    text: ?fn (*Ctx, []const u8, Span) Error![]const u8 = null,
    /// The row's kind from its text, in place of the rule's own.
    kind_of: ?fn ([]const u8) ScalarKind = null,
    /// An extended scalar's kind.
    ext_kind: ?Node.Kind.Extended.ExtKind = null,
};

fn makeScalar(cx: *Ctx, comptime kind: ScalarKind, comptime opts: ScalarOpts, raw: []const u8, span: Span) Error!Node.Id {
    const text = if (opts.text) |f| try f(cx, raw, span) else raw;
    const k = if (opts.kind_of) |f| f(text) else kind;
    const node: Node.Kind = if (opts.ext_kind) |e| .{ .extended = .{ .kind = e, .text = text } } else switch (k) {
        .null_ => .null_,
        .boolean => .{ .boolean = std.mem.eql(u8, text, "true") },
        .int => .{ .number = .{ .raw = text, .kind = .integer } },
        .float => .{ .number = .{ .raw = text, .kind = .float } },
        .string => .{ .string = text },
    };
    return cx.w.add(node, span);
}

/// A scalar of `kind` over what `R` matched.
pub fn scalar(comptime kind: ScalarKind, comptime R: type, comptime opts: ScalarOpts) type {
    return struct {
        pub const Out = Node.Id;
        pub fn match(cx: *Ctx) Error!?Node.Id {
            const span = try R.match(cx) orelse return null;
            return try makeScalar(cx, kind, opts, Span.of(u8, span, cx.sc.src), span);
        }
    };
}

/// A key: a string scalar over what `R` matched.
pub fn key(comptime R: type) type {
    return scalar(.string, R, .{});
}

pub const BareOpts = struct {
    /// Where the value stops, besides the line's end.
    stop: ?type = null,
    trim: bool = true,
    empty: bool = true,
    kind: ScalarKind = .string,
    scalar: ScalarOpts = .{},
};

/// A bare value: to the end of the line, or to where `stop` matches,
/// trimmed of spaces and tabs unless `trim` is off. May be empty unless
/// `empty` is off.
pub fn bare(comptime opts: BareOpts) type {
    return struct {
        pub const Out = Node.Id;
        pub fn match(cx: *Ctx) Error!?Node.Id {
            const s = cx.sc.pos;
            while (cx.sc.byte(0)) |c| {
                if (c == '\n' or c == '\r') break;
                if (opts.stop) |Stop| {
                    const p = cx.sc.pos;
                    const hit = try Stop.match(cx) != null;
                    cx.sc.pos = p;
                    if (hit) break;
                }
                cx.sc.advance(1);
            }
            var ts = s;
            var te = cx.sc.pos;
            if (opts.trim) {
                const b = cx.sc.src;
                while (ts < te and Scanner.isHs(b[ts])) ts += 1;
                while (te > ts and Scanner.isHs(b[te - 1])) te -= 1;
            }
            if (!opts.empty and ts == te) {
                cx.sc.pos = s;
                return null;
            }
            return try makeScalar(cx, opts.kind, opts.scalar, cx.sc.src[ts..te], Span.init(ts, te));
        }
    };
}

/// One escape: the byte after the escape byte, and what it means.
pub const Escape = struct { byte: u8, means: []const u8 };

pub const QuotedOpts = struct {
    open: []const u8,
    /// `open` when null.
    close: ?[]const u8 = null,
    /// Nothing escapes when empty.
    escapes: []const Escape = &.{},
    escape: u8 = '\\',
    /// `\r\n` inside reads as `\n`.
    crlf: bool = true,
    /// A bare `\r` inside refuses the input with this message.
    bare_cr: ?[]const u8 = null,
    unclosed: ?[]const u8 = null,
    bad_escape: ?[]const u8 = null,
    /// Runs once past the closing quote: may refuse what follows on the
    /// line, and may answer the offset a bad escape is reported at.
    after: ?fn (*Ctx, Span) Error!?usize = null,
    kind: ScalarKind = .string,
};

/// A quoted value; its span includes the quotes.
pub fn quoted(comptime opts: QuotedOpts) type {
    const close = opts.close orelse opts.open;
    const unclosed = opts.unclosed orelse "unclosed quoted value; expected a matching `" ++ close ++ "`";
    const escaping = opts.escapes.len > 0;
    return struct {
        pub const Out = Node.Id;
        pub fn match(cx: *Ctx) Error!?Node.Id {
            const s = cx.sc.pos;
            if (cx.sc.lit(opts.open) == null) return null;
            const bytes = cx.sc.src;
            var i = cx.sc.pos;
            while (true) {
                if (i >= bytes.len) return cx.fail(unclosed, i);
                const c = bytes[i];
                if (c == '\r') {
                    if (i + 1 < bytes.len and bytes[i + 1] == '\n') {
                        i += 2;
                    } else if (opts.bare_cr) |m| {
                        return cx.fail(m, i);
                    } else i += 1;
                } else if (escaping and c == opts.escape) {
                    if (i + 1 >= bytes.len) return cx.fail(unclosed, i);
                    i += 2;
                } else if (std.mem.startsWith(u8, bytes[i..], close)) {
                    i += close.len;
                    break;
                } else i += 1;
            }
            cx.sc.pos = i;
            const span = Span.init(s, i);
            const err_at = if (opts.after) |f| (try f(cx, span)) orelse i else i;
            const inner = bytes[s + opts.open.len .. i - close.len];
            // The text is the inner bytes themselves unless something in
            // them reads otherwise.
            const plain = std.mem.indexOfScalar(u8, inner, '\r') == null and
                (!escaping or std.mem.indexOfScalar(u8, inner, opts.escape) == null);
            if (plain) return try makeScalar(cx, opts.kind, .{}, inner, span);
            var out: std.ArrayList(u8) = .empty;
            defer out.deinit(cx.allocator);
            var j: usize = 0;
            while (j < inner.len) {
                const ch = inner[j];
                if (opts.crlf and ch == '\r' and j + 1 < inner.len and inner[j + 1] == '\n') {
                    try out.append(cx.allocator, '\n');
                    j += 2;
                } else if (escaping and ch == opts.escape) {
                    const next = inner[j + 1];
                    const rep = for (opts.escapes) |e| {
                        if (e.byte == next) break e.means;
                    } else {
                        if (opts.bad_escape) |m| return cx.fail(m, err_at);
                        return cx.failFmt(err_at, "invalid escape `{c}{c}` in a quoted value", .{ opts.escape, next });
                    };
                    try out.appendSlice(cx.allocator, rep);
                    j += 2;
                } else {
                    try out.append(cx.allocator, ch);
                    j += 1;
                }
            }
            return try makeScalar(cx, opts.kind, .{}, out.items, span);
        }
    };
}

pub const CommentOpts = struct {
    open: []const u8,
    close: ?[]const u8 = null,
    /// Trim spaces, tabs and `\r` at either end of a line comment, all
    /// whitespace at either end of a block one.
    trim: bool = true,
    /// Block when there is a `close`, else line.
    style: ?AST.Comment.Style = null,
    unclosed: ?[]const u8 = null,
};

/// A comment: `open` to the end of the line, or to `close`.
pub fn comment(comptime opts: CommentOpts) type {
    const style = opts.style orelse if (opts.close != null) AST.Comment.Style.block else AST.Comment.Style.line;
    return struct {
        pub const Out = Comment;
        pub fn match(cx: *Ctx) Error!?Comment {
            const s = cx.sc.pos;
            if (cx.sc.lit(opts.open) == null) return null;
            const body = cx.sc.pos;
            var end: usize = undefined;
            if (opts.close) |c| {
                end = cx.sc.find(c) orelse return cx.fail(opts.unclosed orelse "unclosed comment; expected `" ++ c ++ "`", s);
                cx.sc.pos = end + c.len;
            } else {
                const rest = cx.sc.src[body..];
                end = body + (std.mem.indexOfAny(u8, rest, "\r\n") orelse rest.len);
                cx.sc.pos = end;
            }
            var text = cx.sc.src[body..end];
            if (opts.trim) text = std.mem.trim(u8, text, if (opts.close != null) " \t\n\x0b\x0c\r" else " \t\r");
            return .{ .span = Span.init(s, cx.sc.pos), .text = text, .style = style };
        }
    };
}

pub const TriviaOpts = struct {
    /// Spaces, tabs and line endings by default.
    space: type = choice(.{ hs1, eol }),
    comment: ?type = null,
};

/// What lies between the things that matter: `space` and `comment`, in any
/// order, comments bound as they pass. Always matches.
pub fn trivia(comptime opts: TriviaOpts) type {
    return struct {
        pub const Out = Span;
        pub fn match(cx: *Ctx) Error!?Span {
            const s = cx.sc.pos;
            while (!cx.sc.eof()) {
                const p = cx.sc.pos;
                if (try opts.space.match(cx) != null and cx.sc.pos != p) continue;
                cx.sc.pos = p;
                const C = opts.comment orelse break;
                const c = try C.match(cx) orelse break;
                try cx.comment(c);
            }
            return Span.init(s, cx.sc.pos);
        }
    };
}

pub const EntryOpts = struct {
    /// Matched, and skipped when it does not match.
    prefix: ?type = null,
    key: type,
    /// Around the separator.
    between: type = hs,
    sep: ?type = null,
    value: ?type = null,
    missing_sep: []const u8 = "expected a separator after this key",
    missing_value: []const u8 = "expected a value here",
    /// Which node the waiting comments lead.
    leading: Target = .key,
};

/// A keyvalue: the prefix, the key, the separator and the value. Once the
/// key has matched the rest is expected. Answers the keyvalue's row.
pub fn entry(comptime opts: EntryOpts) type {
    return struct {
        pub const Out = Node.Id;
        pub fn match(cx: *Ctx) Error!?Node.Id {
            const s = cx.sc.pos;
            if (opts.prefix) |P| if (try P.match(cx) == null) {
                cx.sc.pos = s;
            };
            const k = try opts.key.match(cx) orelse {
                cx.sc.pos = s;
                return null;
            };
            _ = try opts.between.match(cx);
            if (opts.sep) |Sep| {
                if (try Sep.match(cx) == null) return cx.failHere(opts.missing_sep);
                _ = try opts.between.match(cx);
            }
            const v = if (opts.value) |V|
                try V.match(cx) orelse return cx.failHere(opts.missing_value)
            else
                try cx.w.add(.null_, Span.init(cx.sc.pos, cx.sc.pos));
            const end = cx.span(v).end;
            const kv = try cx.w.add(.{ .keyvalue = .{ .key = k, .value = v } }, Span.init(cx.span(k).start, end));
            try cx.flush(switch (opts.leading) {
                .key => k,
                .value => v,
                .entry => kv,
            }, table.comment_leading);
            return kv;
        }
    };
}

pub const MapOpts = struct {
    open: ?type = null,
    /// The end of the input when null.
    close: ?type = null,
    entry: type,
    /// Between entries.
    trivia: type = trivia(.{}),
    /// Runs where an entry is expected and none matches, to refuse the
    /// input with a message of its own.
    otherwise: ?fn (*Ctx) Error!void = null,
    expected: []const u8 = "expected an entry here",
    /// Runs after every entry.
    after: ?fn (*Ctx) Error!void = null,
    duplicates: flat_map.DuplicatePolicy = .overwrite,
    /// The span is the whole input, for a root.
    whole: bool = false,
    unclosed: []const u8 = "unclosed mapping; expected its close before the end of the input",
    /// Which of an entry's nodes a comment on its line trails.
    trailing: Target = .value,
};

/// A mapping. Comments still waiting at the close are its dangling ones.
pub fn map(comptime opts: MapOpts) type {
    return struct {
        pub const Out = Node.Id;
        pub fn match(cx: *Ctx) Error!?Node.Id {
            const s = cx.sc.pos;
            if (opts.open) |O| if (try O.match(cx) == null) return null;
            const m = try cx.w.add(.{ .mapping = null }, Span.init(s, s));
            while (true) {
                _ = try opts.trivia.match(cx);
                if (opts.close) |C| {
                    const p = cx.sc.pos;
                    if (try C.match(cx) != null) break;
                    cx.sc.pos = p;
                    if (cx.sc.eof()) return cx.failHere(opts.unclosed);
                } else if (cx.sc.eof()) break;
                const kv = try opts.entry.match(cx) orelse {
                    if (opts.otherwise) |f| try f(cx);
                    return cx.failHere(opts.expected);
                };
                try cx.putEntry(m, kv, opts.duplicates);
                const e = cx.w.kindOf(kv).keyvalue;
                cx.last = switch (opts.trailing) {
                    .key => e.key,
                    .value => e.value,
                    .entry => kv,
                };
                if (opts.after) |f| try f(cx);
            }
            cx.w.setSpan(m, if (opts.whole) Span.init(0, cx.sc.src.len) else Span.init(s, cx.sc.pos));
            try cx.flush(m, table.comment_dangling);
            cx.last = m;
            return m;
        }
    };
}

pub const SequenceOpts = struct {
    open: ?type = null,
    close: ?type = null,
    item: type,
    trivia: type = trivia(.{}),
    otherwise: ?fn (*Ctx) Error!void = null,
    expected: []const u8 = "expected an item here",
    after: ?fn (*Ctx) Error!void = null,
    whole: bool = false,
    unclosed: []const u8 = "unclosed sequence; expected its close before the end of the input",
};

/// A sequence. Each item takes the comments waiting before it as leading.
pub fn sequence(comptime opts: SequenceOpts) type {
    return struct {
        pub const Out = Node.Id;
        pub fn match(cx: *Ctx) Error!?Node.Id {
            const s = cx.sc.pos;
            if (opts.open) |O| if (try O.match(cx) == null) return null;
            const q = try cx.w.add(.{ .sequence = null }, Span.init(s, s));
            while (true) {
                _ = try opts.trivia.match(cx);
                if (opts.close) |C| {
                    const p = cx.sc.pos;
                    if (try C.match(cx) != null) break;
                    cx.sc.pos = p;
                    if (cx.sc.eof()) return cx.failHere(opts.unclosed);
                } else if (cx.sc.eof()) break;
                const v = try opts.item.match(cx) orelse {
                    if (opts.otherwise) |f| try f(cx);
                    return cx.failHere(opts.expected);
                };
                try cx.index.append(cx.allocator, cx.w, q, v);
                try cx.flush(v, table.comment_leading);
                cx.last = v;
                if (opts.after) |f| try f(cx);
            }
            cx.w.setSpan(q, if (opts.whole) Span.init(0, cx.sc.src.len) else Span.init(s, cx.sc.pos));
            try cx.flush(q, table.comment_dangling);
            cx.last = q;
            return q;
        }
    };
}

pub const DocOpts = struct {
    root: type,
    /// Before and after the root.
    trivia: ?type = null,
    /// Skip a byte-order mark.
    bom: bool = false,
    missing: []const u8 = "no document here",
    trailing: []const u8 = "unexpected content after the document",
    comments: Binding = .{},
};

/// A whole document: `root` between `trivia`, then the end of the input.
/// Comments left waiting at the end are the root's dangling comments.
pub fn Document(comptime opts: DocOpts) type {
    return struct {
        /// Write `input`'s rows into `t`, finished. On a refusal,
        /// `error.Refused` with why in `*failure`; its message lives until
        /// the next parse on this thread.
        pub fn parseInto(allocator: Allocator, t: *table.Table, input: []const u8, failure: *?Failure) Error!void {
            var cx: Ctx = .init(allocator, t, input, opts.comments);
            defer cx.deinit();
            errdefer if (cx.failure) |f| {
                failure.* = keep(f);
            };
            if (opts.bom and std.mem.startsWith(u8, input, "\xef\xbb\xbf")) cx.sc.pos = 3;
            if (opts.trivia) |T| _ = try T.match(&cx);
            const r = try opts.root.match(&cx) orelse return cx.failHere(opts.missing);
            if (opts.trivia) |T| _ = try T.match(&cx);
            if (!cx.sc.eof()) return cx.failHere(opts.trailing);
            try cx.flush(r, table.comment_dangling);
            try cx.w.finish(r);
        }
    };
}

threadlocal var kept_message: [256]u8 = undefined;

/// A failure whose message outlives the context that wrote it.
fn keep(f: Failure) Failure {
    const n = @min(f.message.len, kept_message.len);
    @memcpy(kept_message[0..n], f.message[0..n]);
    return .{ .message = kept_message[0..n], .at = f.at };
}

/// The parser of a Zig language whose grammar is `Doc` (a `Document`) and
/// whose dialect type is `T`: what `kit/contract.zig` and fig's harness
/// call, with a refusal's message and offset in its report.
pub fn Parser(comptime Doc: type, comptime T: type) type {
    return struct {
        const Self = @This();
        allocator: Allocator,

        pub const Diagnostic = struct { code: []const u8, offset: usize, end: ?usize = null };
        pub const Report = struct { diag: ?Diagnostic = null };

        pub fn describe(code: []const u8) []const u8 {
            return code;
        }

        pub fn shortLabel(_: []const u8) []const u8 {
            return "here";
        }

        /// Write `input`'s rows into `out`, with a refusal in `report`.
        pub fn parseWithReportInto(out: *table.Table, input: []const u8, _: T, report: *Report) Error!void {
            var failure: ?Failure = null;
            Doc.parseInto(out.allocator, out, input, &failure) catch |e| {
                if (failure) |f| report.diag = .{ .code = f.message, .offset = f.at };
                return e;
            };
        }

        /// `input` as the document fig would adopt.
        pub fn parse(allocator: Allocator, input: []const u8, t: T) Error!Document_ {
            var tbl = table.Table.init(allocator, input);
            defer tbl.deinit();
            var report: Report = .{};
            try parseWithReportInto(&tbl, input, t, &report);
            return RowWriter.adopt(allocator, input, &tbl);
        }
    };
}

test "a grammar reads entries, binds comments and keeps a repeated key's first place" {
    const kv = entry(.{
        .key = key(ident(std.ascii.isAlphabetic, std.ascii.isAlphanumeric)),
        .sep = lit("="),
        .value = bare(.{ .stop = lit(" #") }),
    });
    const Doc = Document(.{
        .root = map(.{ .entry = kv, .whole = true, .trivia = trivia(.{ .comment = comment(.{ .open = "#" }) }) }),
    });
    const src = "# lead\na=1 # one\nb=2\na=3\n# end\n";
    var t = table.Table.init(std.testing.allocator, src);
    defer t.deinit();
    var failure: ?Failure = null;
    try Doc.parseInto(std.testing.allocator, &t, src, &failure);
    var doc = try RowWriter.adopt(std.testing.allocator, src, &t);
    defer doc.deinit(std.testing.allocator);
    const ast = &doc.ast;
    try std.testing.expectEqualStrings("3", (try ast.getValByPath(&.{.{ .key = "a" }})).kind.string);
    try std.testing.expectEqual(@as(usize, 7), ast.nodeCount());

    var bad = table.Table.init(std.testing.allocator, "a=1\n=2\n");
    defer bad.deinit();
    try std.testing.expectError(error.Refused, Doc.parseInto(std.testing.allocator, &bad, "a=1\n=2\n", &failure));
    try std.testing.expectEqual(@as(usize, 4), failure.?.at);
    try std.testing.expectEqualStrings("expected an entry here", failure.?.message);
}
