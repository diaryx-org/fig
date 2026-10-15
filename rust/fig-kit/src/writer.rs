//! Port of kit/src/writer.zig.
//!
//! A printer's output, which knows the options it was given: `nl` and
//! `indent` write nothing when `pretty` is off, and a level of indentation
//! is `indent_width` spaces; `comments` writes a node's comments as whole
//! lines after a leader. The writer every kit has — the JavaScript kit's
//! `Writer`, Rust's `fig::kit::print::Writer` — over a byte buffer (Zig's is
//! over a `std.Io.Writer`; a `Vec` cannot fail, so nothing here returns an
//! error).

use super::ast::Comment;

#[derive(Debug)]
pub struct Writer<'a> {
    pub out: &'a mut Vec<u8>,
    pub pretty: bool,
    /// Spaces per level of indentation.
    pub indent_width: usize,
}

impl<'a> Writer<'a> {
    pub fn init(out: &'a mut Vec<u8>, pretty: bool, indent_width: usize) -> Writer<'a> {
        Writer { out, pretty, indent_width }
    }

    pub fn put(&mut self, s: &[u8]) {
        self.out.extend_from_slice(s);
    }

    /// End the line, when pretty.
    pub fn nl(&mut self) {
        if self.pretty {
            self.out.push(b'\n');
        }
    }

    /// Indent `depth` levels, when pretty.
    pub fn indent(&mut self, depth: usize) {
        if self.pretty {
            let n = depth * self.indent_width;
            self.out.resize(self.out.len() + n, b' ');
        }
    }

    /// Indent, write `s`, end the line.
    pub fn line(&mut self, depth: usize, s: &[u8]) {
        self.indent(depth);
        self.put(s);
        self.nl();
    }

    /// Write `cs` as whole lines, each line of each comment's text on its own
    /// and trimmed, after `leader` and a space (`# text`, or the bare leader
    /// for an empty line), indented `depth` levels. Answers whether anything
    /// was written.
    pub fn comments(&mut self, cs: &[Comment<'_>], leader: &[u8], depth: usize) -> bool {
        let mut wrote = false;
        for c in cs {
            for l in c.text.split(|&b| b == b'\n') {
                let t = trim_hs(l);
                self.indent(depth);
                self.put(leader);
                if !t.is_empty() {
                    self.out.push(b' ');
                    self.put(t);
                }
                self.out.push(b'\n');
                wrote = true;
            }
        }
        wrote
    }
}

/// `std.mem.trim(u8, s, " \t")`.
fn trim_hs(s: &[u8]) -> &[u8] {
    let is_hs = |c: &u8| *c == b' ' || *c == b'\t';
    let start = s.iter().position(|c| !is_hs(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !is_hs(c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writer_indents_and_writes_comment_lines() {
        let mut buf = Vec::new();
        let mut w = Writer::init(&mut buf, true, 2);
        w.comments(&[Comment::line(b"two\n lines ")], b"#", 1);
        w.line(1, b"a = 1");
        assert_eq!(b"  # two\n  # lines\n  a = 1\n", &buf[..]);
        let mut flat = Vec::new();
        let mut f = Writer::init(&mut flat, false, 2);
        f.line(3, b"x");
        assert_eq!(b"x", &flat[..]);
    }
}
