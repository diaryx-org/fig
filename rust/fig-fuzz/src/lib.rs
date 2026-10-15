//! fig's fuzz targets, as plain functions over a byte slice.
//!
//! Each target's body lives here rather than in its `fuzz_target!` so that
//! two things run the same code: `cargo test`, which runs every target over
//! [`detect_seeds`] and [`canonical_seeds`] on every pull request (that is
//! what stops a target rotting between nightly runs; it is not fuzz
//! coverage), and the cargo-fuzz project at `fuzz/`, which hands each one
//! libFuzzer's inputs (`cargo +nightly fuzz run <target>`). A target fails by
//! panicking, which libFuzzer reports as a crash and `cargo test` as a
//! failed test.
//!
//! The two targets are split by what they can prove:
//!
//! - [`detect`] is breadth. Detection tries every registered parser over the
//!   same bytes, so one target sweeps all ten languages' tokenizers and
//!   parsers for panics, overflowed stacks and hangs. It cannot check
//!   correctness: its result is a format name or none, and each parse's
//!   Document is thrown away.
//! - [`canonical_round_trip`] is depth. The canonical form is a bijection
//!   with the AST, the one property strong enough to catch a silent
//!   mis-parse (a parser that accepts input and builds the wrong tree),
//!   which `detect` would never notice.
//!
//! Neither asserts `parse -> print == original`: printers render from the
//! AST, not the source, so byte-identical output is not a property of fig,
//! and asserting it would only encode today's formatting. The reprint check
//! is idempotence, a weaker and true claim.
//!
//! Leaks are not asserted: ownership frees each Document on every path, the
//! error paths included, so there is nothing for a leak check to find that
//! the compiler has not already ruled out.

use std::sync::OnceLock;

use fig_core::canonical;
use fig_core::kit::language::Language;
use fig_core::registry::{self, Registry};

/// The longest input the fuzz targets take, for `-max_len`. Big enough to
/// nest past the parsers' depth guards (512 for canonical), so the
/// recursion limits themselves get exercised, and to hold a document with
/// real structure rather than one truncated token; small enough that
/// libFuzzer spends its time on shapes, not on length.
pub const MAX_INPUT: usize = 2048;

/// fig's ten languages, in the bundle's order.
fn languages() -> [&'static dyn Language; 10] {
    [
        &fig_json::LANGUAGE,
        &fig_yaml::LANGUAGE,
        &fig_toml::LANGUAGE,
        &fig_zon::LANGUAGE,
        &fig_figl::LANGUAGE,
        &fig_ini::LANGUAGE,
        &fig_dotenv::LANGUAGE,
        &fig_properties::LANGUAGE,
        &fig_plist::LANGUAGE,
        &fig_nestedtext::LANGUAGE,
    ]
}

/// A registry of the ten, built once per process: libFuzzer calls a target
/// in a tight loop, and registering is far dearer than one detection. A
/// registry of its own rather than `registry::global()`, so nothing else in
/// the process can change what is swept.
fn bundle() -> &'static (Registry, Vec<&'static str>) {
    static BUNDLE: OnceLock<(Registry, Vec<&'static str>)> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let reg = Registry::new();
        registry::register_bundle(&reg, &languages()).expect("fig's ten languages register");
        (reg, registry::all_dialects(&languages()))
    })
}

/// Breadth target: detect `input` among every bundled dialect. The
/// assertion is everything around the call: no panic, no stack overflow,
/// and, under libFuzzer's `-timeout`, no hang. The result is ignored because
/// detecting nothing in random bytes is the expected case, not a failure.
pub fn detect(input: &[u8]) {
    let (reg, dialects) = bundle();
    let _ = reg.detect_among(input, dialects);
}

/// Depth target: for any input the canonical parser accepts, printing it and
/// reparsing the print must land on the same tree, comments included, and
/// printing that tree again must give the same bytes.
///
/// The property is conditional on the first parse: a rejected input proves
/// nothing about a bijection, so it returns (the depth guard's
/// `NestingTooDeep` lands there too, which is the guard working). What it
/// does not forgive is anything after: the parser's depth guard matches the
/// printer's, so whatever parsed prints, and the print is canonical text by
/// construction, so if it fails to reparse the bijection is broken.
pub fn canonical_round_trip(input: &[u8]) {
    let Ok(ast) = canonical::parse_abstract(input) else { return };

    let mut first = Vec::new();
    if let Err(e) = canonical::print(&mut first, &ast) {
        panic!("a parsed tree failed to print: {e:?}");
    }

    let reparsed = match canonical::parse_abstract(&first) {
        Ok(ast) => ast,
        Err(e) => panic!("the printer's output does not reparse: {e:?}\n{}", String::from_utf8_lossy(&first)),
    };

    // The bijection itself: same tree in, same tree out.
    assert!(ast.eql(&reparsed), "the reparsed tree differs from the parsed one:\n{}", String::from_utf8_lossy(&first));
    // `eql` ignores comments by design, so they need their own check or the
    // canonical form would be free to drop them silently.
    assert!(
        ast.comments_eql(&reparsed),
        "the reparsed tree's comments differ from the parsed one's:\n{}",
        String::from_utf8_lossy(&first)
    );

    // Idempotence, from the second print on: the first print normalizes
    // whatever spelling the input used (spacing, comment placement), so
    // comparing it to the input would be false. The canonical form has one
    // spelling per tree, so re-rendering the reparsed tree must match the
    // first print byte for byte.
    let mut second = Vec::new();
    if let Err(e) = canonical::print(&mut second, &reparsed) {
        panic!("a reparsed tree failed to print: {e:?}");
    }
    assert_eq!(
        String::from_utf8_lossy(&first),
        String::from_utf8_lossy(&second),
        "printing the reparsed tree changed the canonical text"
    );
}

/// Inputs every target sees on every `cargo test`, and the corpus a fuzz run
/// starts from (`fig-fuzz-seed`): each language's own samples, which are
/// what its registration already checks it parses, so they track the
/// grammars without a fixture list here to keep in step; plus the edges a
/// scanner tends to run off, which no sample holds.
pub fn detect_seeds() -> Vec<Vec<u8>> {
    let mut seeds: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"\n".to_vec(),
        b"\0".to_vec(),
        b"\xff\xfe".to_vec(),
        b"\xe2\x82".to_vec(), // a UTF-8 sequence cut short
        b"\"".to_vec(),
        b"\"\\".to_vec(),
        b"[".to_vec(),
        b"{".to_vec(),
        b"<".to_vec(),
        b"a = \"unterminated\n".to_vec(),
        b"key:\n  - item\n\t- tab\n".to_vec(),
        b"&a [*a]\n".to_vec(),
        b"[a.b]\nc.d = 1\n".to_vec(),
        "[".repeat(600).into_bytes(),
        "- ".repeat(600).into_bytes(),
        // YAML once hung on a NUL after a flow `:`.
        b"{:\0".to_vec(),
        b"{a: 1, b:\0\0\0\0\0\0\0\x02*".to_vec(),
        // ...and on a lone `\r` inside flow.
        b"[O\r[O\r".to_vec(),
        b"{a:\r}".to_vec(),
    ];
    for lang in languages() {
        seeds.extend(lang.samples().iter().map(|s| s.to_vec()));
    }
    seeds
}

/// The canonical target's seeds: every language sample that parses, printed
/// as canonical text, so between them they hold every tree shape the ten
/// grammars build; plus hand-written canonical text for what no sample need
/// hold (extended scalars, anchors, tags, the kind override, comments in
/// every slot, nesting at and past the depth guard).
pub fn canonical_seeds() -> Vec<Vec<u8>> {
    let mut seeds: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"null".to_vec(),
        b"~i0xFF".to_vec(),
        b"~f1".to_vec(),
        b"\"tab:\\t quote:\\\" \\u+041\"".to_vec(),
        b"{\n  \"dt\": @offset_datetime \"1979-05-27T07:32:00Z\",\n  \"d\": @local_date \"1979-05-27\",\n  \"mode\": @enum_literal \"fast\",\n  \"ch\": @char_literal \"65\",\n  \"inf\": @number_special \"Infinity\"\n}".to_vec(),
        b"[&base { \"retries\": 3 }, *base, !!str \"tagged\"]".to_vec(),
        b"{\n  // leading\n  \"a\": /* block */ 1, // trailing\n  \"b\": [2 /* c3 */],\n  \"c\": {}, // on empty\n  // dangling\n}".to_vec(),
        format!("{}{}", "[".repeat(512), "]".repeat(512)).into_bytes(),
        format!("{}{}", "[".repeat(513), "]".repeat(513)).into_bytes(),
    ];
    let (reg, dialects) = bundle();
    for lang in languages() {
        for sample in lang.samples() {
            for name in dialects {
                let Some(entry) = reg.entry_by_name(name) else { continue };
                if !std::ptr::addr_eq(entry.lang(), lang) {
                    continue;
                }
                if let Ok(doc) = entry.parse(sample) {
                    let mut text = Vec::new();
                    if canonical::print(&mut text, &doc.ast).is_ok() {
                        seeds.push(text);
                        break;
                    }
                }
            }
        }
    }
    seeds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_never_panics_on_the_seeds() {
        for seed in detect_seeds() {
            detect(&seed);
        }
    }

    #[test]
    fn canonical_print_output_reparses_to_an_equal_ast_on_the_seeds() {
        let seeds = canonical_seeds();
        // Ten hand-written, and at least one printed sample per language: if
        // the samples stopped printing, this would pass on the hand-written
        // few alone and say nothing about the ten grammars' trees.
        assert!(seeds.len() >= 20, "only {} canonical seeds", seeds.len());
        for seed in seeds {
            canonical_round_trip(&seed);
        }
    }
}
