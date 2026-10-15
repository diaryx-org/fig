//! The languages fig's own unit tests read and write, each registered as its
//! language crate registers it — `fig-core`'s own implementation, through
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
        use fig_core::lang;
        let languages: [&'static dyn fig_core::kit::language::Language; 10] = [
            &lang::json::LANGUAGE,
            &lang::yaml::LANGUAGE,
            &lang::toml::LANGUAGE,
            &lang::zon::LANGUAGE,
            &lang::figl::LANGUAGE,
            &lang::ini::LANGUAGE,
            &lang::dotenv::LANGUAGE,
            &lang::properties::LANGUAGE,
            &lang::plist::LANGUAGE,
            &lang::nestedtext::LANGUAGE,
        ];
        for language in languages {
            crate::language::register_core(language).expect("a language registers");
        }
    });
    Format::by_name(name).unwrap_or_else(|| panic!("no format named {name:?}"))
}
