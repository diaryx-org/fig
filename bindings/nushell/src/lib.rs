//! A nushell plugin for the fig authoring dialect (`.figl`).
//!
//! Two commands, `from figl` and `to figl`. Registering `from figl` is also what
//! makes `open config.figl` return structured data, since nushell's `open`
//! dispatches on file extension to a `from <ext>` command.
//!
//! The plugin talks to fig through the native `fig` crate rather than shelling
//! out to the CLI, which is what lets it map figl's datetimes and nulls onto
//! nushell's own types in one pass — see [`convert`] for why no
//! convert-through-JSON shim can do both.
//!
//! The library target exists so the integration tests can drive the commands
//! in-process through `nu-plugin-test-support`; `main.rs` is a thin `serve_plugin`
//! wrapper over it.

pub mod convert;
pub mod from_figl;
pub mod to_figl;

use nu_plugin::{Plugin, PluginCommand};

pub struct FigPlugin;

impl Plugin for FigPlugin {
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }

    fn commands(&self) -> Vec<Box<dyn PluginCommand<Plugin = Self>>> {
        vec![Box::new(from_figl::FromFigl), Box::new(to_figl::ToFigl)]
    }
}
