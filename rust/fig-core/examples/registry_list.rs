//! Lists the bundled registry's dialects, and the refusal if registering the
//! bundle stopped early.
fn main() {
    let r = fig_core::registry::global();
    for e in r.entries() {
        println!("{} ({})", e.name, e.lang().name());
    }
    let why = fig_core::registry::last_refusal();
    if !why.is_empty() {
        println!("refused: {}", String::from_utf8_lossy(&why));
    }
}
