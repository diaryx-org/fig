//! Port of kit/src/json_string.zig: the shared JSON string encoder —
//! double-quote and escape a byte slice using the JSON escape set.
//!
//! This is the single source of truth for the *encode* side, used by both the
//! JSON printer and the canonical printer. The escape set here is the inverse
//! of what each parser's escape *decoder* accepts — anything emitted here must
//! decode back unchanged, so keep the two in lockstep.

/// Write `value` as a quoted, escaped JSON string (surrounding `"` included).
/// Bytes ≥ 0x20 other than `"`/`\` pass through verbatim, so arbitrary UTF-8
/// round-trips. Runs of pass-through bytes are flushed at once, dropping to the
/// escape path only at a byte that needs it.
pub fn write_quoted(writer: &mut Vec<u8>, value: &[u8]) {
    write_quoted_in(writer, value, b'"')
}

/// `write_quoted` between `quote`s: `"`, or the `'` JSON5 also takes, which
/// escapes a `'` and lets a `"` through instead. Panics on any other `quote`
/// (a compile error in Zig).
pub fn write_quoted_in(writer: &mut Vec<u8>, value: &[u8], quote: u8) {
    assert!(quote == b'"' || quote == b'\'', "write_quoted_in: quote must be '\"' or '\\''");
    writer.push(quote);
    let mut run_start = 0;
    for (i, &char) in value.iter().enumerate() {
        let escape: &[u8] = match char {
            b'"' | b'\'' => {
                if char != quote {
                    continue;
                } else if quote == b'"' {
                    b"\\\""
                } else {
                    b"\\'"
                }
            }
            b'\\' => b"\\\\",
            0x08 => b"\\b",
            0x0c => b"\\f",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0x00..=0x07 | 0x0b | 0x0e..=0x1f => {
                writer.extend_from_slice(&value[run_start..i]);
                write_control_escape(writer, char);
                run_start = i + 1;
                continue;
            }
            _ => continue, // pass-through: extend the current run
        };
        writer.extend_from_slice(&value[run_start..i]);
        writer.extend_from_slice(escape);
        run_start = i + 1;
    }
    writer.extend_from_slice(&value[run_start..]);
    writer.push(quote);
}

/// Emit a non-printable control byte as a `\u00XX` escape.
fn write_control_escape(writer: &mut Vec<u8>, char: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    writer.extend_from_slice(b"\\u00");
    writer.push(HEX[(char >> 4) as usize]);
    writer.push(HEX[(char & 0x0f) as usize]);
}

/// How long `write_quoted_in`'s answer can be: every byte a six-byte `\u00XX`,
/// and the two quotes. From kit/src/rules.zig (`jsonStringRoom`,
/// `FIG_KIT_JSON_STRING_ROOM`).
pub fn json_string_room(len: usize) -> usize {
    len * 6 + 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted_in(value: &[u8], quote: u8) -> Vec<u8> {
        let mut out = Vec::new();
        write_quoted_in(&mut out, value, quote);
        out
    }

    #[test]
    fn passes_through_escapes_and_quotes() {
        let mut out = Vec::new();
        write_quoted(&mut out, b"tab:\t quote:\" back:\\ ctrl:\x07 utf:\xc3\xa9");
        assert_eq!(out, b"\"tab:\\t quote:\\\" back:\\\\ ctrl:\\u0007 utf:\xc3\xa9\"".to_vec());
    }

    #[test]
    fn single_quotes_escape_the_other_way() {
        assert_eq!(quoted_in(b"it's \"x\"", b'\''), b"'it\\'s \"x\"'".to_vec());
        assert_eq!(quoted_in(b"it's \"x\"", b'"'), b"\"it's \\\"x\\\"\"".to_vec());
    }

    #[test]
    fn every_control_byte() {
        assert_eq!(quoted_in(b"", b'"'), b"\"\"".to_vec());
        assert_eq!(quoted_in(b"\x00\x0b\x0e\x1f\x7f", b'"'), b"\"\\u0000\\u000b\\u000e\\u001f\x7f\"".to_vec());
        assert_eq!(quoted_in(b"\x08\x0c\n\r\t", b'"'), b"\"\\b\\f\\n\\r\\t\"".to_vec());
        // The room is never too small.
        let all: Vec<u8> = (0..=0x1f).collect();
        assert!(quoted_in(&all, b'"').len() <= json_string_room(all.len()));
    }
}
