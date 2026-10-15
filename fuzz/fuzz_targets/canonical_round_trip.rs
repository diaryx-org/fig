//! The canonical form's bijection with the AST: what parses prints, reparses
//! to an equal tree with equal comments, and reprints identically
//! (`fig_fuzz::canonical_round_trip`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `-max_len` bounds what libFuzzer generates; this bounds what a corpus
    // file handed in by hand can make a run spend.
    if data.len() <= fig_fuzz::MAX_INPUT {
        fig_fuzz::canonical_round_trip(data);
    }
});
