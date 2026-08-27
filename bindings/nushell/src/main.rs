use nu_plugin::{MsgPackSerializer, serve_plugin};
use nu_plugin_fig::FigPlugin;

fn main() {
    // MsgPack rather than JSON: it is nushell's default plugin encoding and the
    // faster of the two, and nothing here needs a human-readable wire format.
    serve_plugin(&FigPlugin, MsgPackSerializer);
}
