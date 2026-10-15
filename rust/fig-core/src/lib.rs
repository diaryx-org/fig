//! fig in Rust: edit config files losslessly, and convert between formats
//! keeping as much as each can hold. A port of fig's Zig core, kit and
//! languages (docs/proposals/rust-core.md), file for file.

// The kit and the error type are `fig-kit`'s, kept at their old paths here:
// `fig_core::kit::ast`, `fig_core::Error`, `fig_core::err!`.
pub use fig_kit as kit;
pub use fig_kit::error;
pub use fig_kit::{err, Error, Result};

pub mod api;
pub mod canonical;
pub mod deserialize;
pub mod diagnostics;
pub mod editor;
pub mod flat_strip;
pub mod lang;
pub mod lossless;
pub mod materialize;
pub mod patch;
pub mod registry;
pub mod serialize;
pub mod wire;
pub mod host;
