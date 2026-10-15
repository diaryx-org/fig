//! A helper executable: dotenv written with `fig::kit` (`language.rs`),
//! served over stdio for a `fig` binary to spawn, and held to the compiled
//! `dotenv` by `fig lang check kit-dotenv --against dotenv`
//! (`tools/cli-lang-check.sh`). An argument renames it.
//!
//! It needs the crate's `helper` feature: `cargo build --features helper
//! --example kit_dotenv_helper`.

mod language;

fn main() -> std::io::Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| "kit-dotenv".to_owned());
    fig::helper::serve(language::KitDotenv { name })
}
