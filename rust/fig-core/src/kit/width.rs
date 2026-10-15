//! Port of kit/src/width.zig: rendered-width measurement shared by the format
//! printers' inline-layout decisions. The one honest way to know how wide a
//! value will be is to emit it with the very function that produces the real
//! output, against a writer that only counts — so the estimate can never drift
//! from the bytes actually written (the alternative, hand-summed arithmetic
//! widths, silently rots the moment the emitter's spelling changes). TOML and
//! YAML both measure this way; fig keeps its own arithmetic pass for now.

/// Byte width of whatever `render` writes, or `None` if `render` errors — which
/// callers read as "no inline spelling, keep it block". Zig's
/// `rendered(render, args)` calls `render(writer, args...)`; here `render` is a
/// closure handed the writer:
///
/// ```text
/// width::rendered(|w| write_inline(w, ast, id)) // measures write_inline(w, ast, id)
/// ```
pub fn rendered<E, F>(render: F) -> Option<usize>
where
    F: FnOnce(&mut Vec<u8>) -> core::result::Result<(), E>,
{
    let mut out = Vec::new();
    render(&mut out).ok()?;
    Some(out.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Result, err};

    fn emit(w: &mut Vec<u8>, s: &[u8]) -> Result<()> {
        w.extend_from_slice(s);
        Ok(())
    }

    #[test]
    fn counts_what_render_writes_and_none_on_error() {
        assert_eq!(rendered(|w| emit(w, b"hello")), Some(5));
        // Longer than Zig's 64-byte scratch buffer: still every byte counted.
        assert_eq!(rendered(|w| emit(w, &[b'x'; 1000])), Some(1000));
        assert_eq!(rendered(|w| emit(w, b"")), Some(0));
        let failing = |w: &mut Vec<u8>| -> Result<()> {
            w.push(b'x');
            Err(err!(NonFiniteNumber))
        };
        assert_eq!(rendered(failing), None);
    }
}
