//! Detection over every bundled language: no panic, no stack overflow, no
//! hang (`fig_fuzz::detect`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `-max_len` bounds what libFuzzer generates; this bounds what a corpus
    // file handed in by hand can make a run spend.
    if data.len() <= fig_fuzz::MAX_INPUT {
        fig_fuzz::detect(data);
    }
});
