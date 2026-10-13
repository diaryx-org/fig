//! Every language fig ships, registered once per test binary from its
//! crate, and a format found by name after.
#![allow(dead_code)]

use std::sync::Once;

/// Register every language fig ships.
pub fn register_all() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        fig_json::register().expect("json registers");
        fig_yaml::register().expect("yaml registers");
        fig_toml::register().expect("toml registers");
        fig_zon::register().expect("zon registers");
        fig_figl::register().expect("figl registers");
        fig_ini::register().expect("ini registers");
        fig_dotenv::register().expect("dotenv registers");
        fig_properties::register().expect("properties registers");
        fig_plist::register().expect("plist registers");
        fig_nestedtext::register().expect("nestedtext registers");
    });
}

/// The registered format named `name`, every language registered first.
pub fn by_name(name: &str) -> fig::Format {
    register_all();
    fig::Format::by_name(name).unwrap_or_else(|| panic!("no format named {name:?}"))
}
