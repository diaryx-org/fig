//! Write the fuzz targets' seed inputs out as libFuzzer corpora:
//! `fig-fuzz-seed <dir>` writes `<dir>/detect/seed-<n>` and
//! `<dir>/canonical_round_trip/seed-<n>`, one file per seed, so a fuzz run
//! pointed at `fuzz/corpus` starts from inputs every parser walks deep into
//! rather than from the empty input. Rewriting the same names makes it safe
//! to run over a corpus libFuzzer has already grown; it adds nothing twice
//! and removes nothing.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: fig-fuzz-seed <corpus-dir>");
        return ExitCode::from(2);
    };
    for (target, seeds) in [("detect", fig_fuzz::detect_seeds()), ("canonical_round_trip", fig_fuzz::canonical_seeds())] {
        let at = dir.join(target);
        let wrote = std::fs::create_dir_all(&at).and_then(|()| {
            seeds.iter().enumerate().try_for_each(|(n, seed)| std::fs::write(at.join(format!("seed-{n}")), seed))
        });
        if let Err(e) = wrote {
            eprintln!("fig-fuzz-seed: {}: {e}", at.display());
            return ExitCode::FAILURE;
        }
        println!("{}: {} seeds", at.display(), seeds.len());
    }
    ExitCode::SUCCESS
}
