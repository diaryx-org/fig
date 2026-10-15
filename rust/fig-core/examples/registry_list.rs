//! Registers fig's ten languages, from their crates, and lists the
//! registry's dialects, and the refusal if registering them stopped early.
fn main() {
    let r = fig_core::registry::global();
    let _ = fig_core::registry::register_bundle(
        r,
        &[
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
        ],
    );
    for e in r.entries() {
        println!("{} ({})", e.name, e.lang().name());
    }
    let why = fig_core::registry::last_refusal();
    if !why.is_empty() {
        println!("refused: {}", String::from_utf8_lossy(&why));
    }
}
