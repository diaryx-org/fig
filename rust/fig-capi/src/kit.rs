//! The kit's rules: `fig_kit_*` (`kit/src/rules.zig`), for a language that
//! is not Zig — how a number is spelled, which datetime a token is, what a
//! bare token means, a JSON string's quoting, and the XML tokenizer. Each
//! reads `len` bytes and keeps nothing; the rule is fig-core's kit.

use fig_core::kit::literal::{self, NumberKind, Sniffed};
use fig_core::kit::table::Kind;
use fig_core::kit::{datetime, json_string, number, xml};

use crate::abi::FigXmlToken;
use crate::slice_of;

/// `len` bytes at `raw`, the empty slice for zero.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes when `len` is nonzero.
unsafe fn input<'a>(raw: *const u8, len: usize) -> &'a [u8] {
    // SAFETY: the caller's contract; a null pointer with a length is read as
    // nothing rather than dereferenced.
    unsafe { slice_of(raw, len) }.unwrap_or(&[])
}

/// `answer` into `out[..out_cap]`, its length, or `usize::MAX` when it
/// does not fit.
///
/// # Safety
///
/// `out` must be writable for `out_cap` bytes.
unsafe fn answer(bytes: &[u8], out: *mut u8, out_cap: usize) -> usize {
    if bytes.len() > out_cap || (out.is_null() && !bytes.is_empty()) {
        return usize::MAX;
    }
    if !bytes.is_empty() {
        // SAFETY: `out` is writable for `out_cap >= bytes.len()` bytes.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}

/// `fig_kit_number_spellable`: whether a format that spells `spelling`
/// (`FIG_SPELL_*`) reads `raw` back as the same number.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_number_spellable(raw: *const u8, len: usize, spelling: u32) -> bool {
    // SAFETY: the caller's contract.
    number::spellable(unsafe { input(raw, len) }, number::spelling(spelling))
}

/// `fig_kit_number_canonical`: `raw`'s decimal lexeme into `out`, its
/// length, or `SIZE_MAX` when `out_cap` is too small.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes, `out` writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_number_canonical(raw: *const u8, len: usize, out: *mut u8, out_cap: usize) -> usize {
    let mut w = Vec::new();
    // SAFETY: the caller's contract.
    number::write_canonical(&mut w, unsafe { input(raw, len) });
    // SAFETY: the caller's contract.
    unsafe { answer(&w, out, out_cap) }
}

/// `fig_kit_number_non_finite`: `FIG_NON_FINITE_*`, or -1.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_number_non_finite(raw: *const u8, len: usize) -> i32 {
    // SAFETY: the caller's contract.
    number::non_finite(unsafe { input(raw, len) }).map_or(-1, |v| v as i32)
}

/// `fig_kit_datetime_classify`: the `FigExtKind` of the datetime `raw` is,
/// or -1.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_datetime_classify(raw: *const u8, len: usize, flags: u32) -> i32 {
    // SAFETY: the caller's contract.
    datetime::classify(unsafe { input(raw, len) }, datetime::options(flags)).map_or(-1, |k| k as i32)
}

/// `fig_kit_datetime_valid_date`.
///
/// # Safety
///
/// `s` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_datetime_valid_date(s: *const u8, len: usize) -> bool {
    // SAFETY: the caller's contract.
    datetime::validate_date(unsafe { input(s, len) }).is_ok()
}

/// `fig_kit_datetime_valid_time`.
///
/// # Safety
///
/// `s` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_datetime_valid_time(s: *const u8, len: usize, flags: u32) -> bool {
    // SAFETY: the caller's contract.
    datetime::validate_time(unsafe { input(s, len) }, datetime::options(flags)).is_ok()
}

/// `fig_kit_datetime_valid_offset`.
///
/// # Safety
///
/// `s` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_datetime_valid_offset(s: *const u8, len: usize) -> bool {
    // SAFETY: the caller's contract.
    datetime::validate_offset(unsafe { input(s, len) }).is_ok()
}

/// `fig_kit_literal_kind`: the `FigRow` kind a bare token is written as.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_literal_kind(raw: *const u8, len: usize) -> u8 {
    // SAFETY: the caller's contract.
    match literal::sniff_bare(unsafe { input(raw, len) }) {
        Sniffed::Null => Kind::NULL,
        Sniffed::Boolean(true) => Kind::TRUE,
        Sniffed::Boolean(false) => Kind::FALSE,
        Sniffed::Number(n) => {
            if n.kind == NumberKind::Float {
                Kind::FLOAT
            } else {
                Kind::INT
            }
        }
        Sniffed::Datetime { kind, .. } => Kind::EXT + kind as u8,
        Sniffed::String => Kind::STRING,
    }
}

/// `fig_kit_literal_number`: `FIG_ROW_INT` or `FIG_ROW_FLOAT`, or -1.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_literal_number(raw: *const u8, len: usize) -> i32 {
    // SAFETY: the caller's contract.
    match literal::sniff_number(unsafe { input(raw, len) }) {
        Some(n) if n.kind == NumberKind::Float => Kind::FLOAT as i32,
        Some(_) => Kind::INT as i32,
        None => -1,
    }
}

/// `fig_kit_json_string_quote`: `raw` between `quote`s, JSON-escaped, into
/// `out`; its length, or `SIZE_MAX` when `out_cap` is too small or `quote`
/// is neither `"` nor `'`.
///
/// # Safety
///
/// `raw` must be readable for `len` bytes, `out` writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_json_string_quote(raw: *const u8, len: usize, quote: u8, out: *mut u8, out_cap: usize) -> usize {
    if quote != b'"' && quote != b'\'' {
        return usize::MAX;
    }
    let mut w = Vec::new();
    // SAFETY: the caller's contract.
    json_string::write_quoted_in(&mut w, unsafe { input(raw, len) }, quote);
    // SAFETY: the caller's contract.
    unsafe { answer(&w, out, out_cap) }
}

/// `xml_errors`: a refusal's `FIG_XML_ERR_*`, 0 for none of them.
fn xml_error_code(name: &str) -> i32 {
    const NAMES: [&str; 8] = [
        "UnclosedTag",
        "UnclosedAttributeValue",
        "UnclosedComment",
        "UnclosedCData",
        "UnclosedPI",
        "UnsupportedDoctype",
        "InvalidMarkup",
        "UnexpectedCharacter",
    ];
    NAMES.iter().position(|n| *n == name).map_or(0, |i| i as i32 + 1)
}

/// `fig_kit_xml_tokenize`: up to `cap` tokens of the `len` bytes at `xml`
/// into `out`, and how many there are; `SIZE_MAX` on a refusal, with its
/// `FIG_XML_ERR_*` in `*err` and where the tokenizer stopped in `*err_at`.
///
/// # Safety
///
/// `xml` must be readable for `len` bytes; `out` writable for `cap` tokens
/// (or null with `cap` 0); `err` and `err_at` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fig_kit_xml_tokenize(xml: *const u8, len: usize, out: *mut FigXmlToken, cap: usize, err: *mut i32, err_at: *mut usize) -> usize {
    const FAIL: usize = usize::MAX;
    // SAFETY: the caller's contract.
    let Some(bytes) = (unsafe { slice_of(xml, len) }) else { return FAIL };
    if len > u32::MAX as usize {
        return FAIL;
    }
    let mut tz = xml::Tokenizer::new(bytes);
    let tokens = match tz.tokenize() {
        Ok(t) => t,
        Err(e) => {
            // SAFETY: each null or writable by the caller's word.
            unsafe {
                crate::put(err, xml_error_code(e.name()));
                crate::put(err_at, tz.pos);
            }
            return FAIL;
        }
    };
    if !out.is_null() {
        for (i, t) in tokens.iter().take(cap).enumerate() {
            let kind = match t.kind {
                xml::Kind::Lt => 0,
                xml::Kind::LtSlash => 1,
                xml::Kind::Gt => 2,
                xml::Kind::SlashGt => 3,
                xml::Kind::Eq => 4,
                xml::Kind::Name => 5,
                xml::Kind::AttrValue => 6,
                xml::Kind::CharData => 7,
                xml::Kind::Cdata => 8,
                xml::Kind::Eof => 9,
            };
            // SAFETY: `out` is writable for `cap` tokens and `i < cap`.
            unsafe { out.add(i).write(FigXmlToken { kind, reserved: [0; 3], start: t.span.start as u32, end: t.span.end as u32 }) };
        }
    }
    tokens.len()
}
