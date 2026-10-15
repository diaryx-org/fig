//! The languages fig's own unit tests read and write, each registered as its
//! language crate registers it — the crate's `LANGUAGE`, through
//! [`crate::language::register_core`] — since the crates' `register()`
//! returns another copy of this crate's `Format`, a unit test's being the
//! crate under test.

use crate::Format;
use std::sync::Once;

/// The registered format named `name`, every language fig ships registered
/// first.
pub fn by_name(name: &str) -> Format {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let languages: [&'static dyn fig_core::kit::language::Language; 10] = [
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
        ];
        for language in languages {
            crate::language::register_core(language).expect("a language registers");
        }
    });
    Format::by_name(name).unwrap_or_else(|| panic!("no format named {name:?}"))
}
