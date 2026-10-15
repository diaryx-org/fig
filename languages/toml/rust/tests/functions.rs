//! The functions every language crate has, as TOML has them: no
//! `register()` first, typed values through serde both ways.

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Config {
    name: String,
    port: u16,
}

const TEXT: &str = "name = \"fig\"\nport = 8080\n";

fn config() -> Config {
    Config {
        name: "fig".into(),
        port: 8080,
    }
}

#[test]
fn reads_with_nothing_registered_first() {
    assert_eq!(fig_toml::from_str::<Config>(TEXT).unwrap(), config());
    assert_eq!(
        fig_toml::from_slice::<Config>(TEXT.as_bytes()).unwrap(),
        config()
    );
    assert_eq!(
        fig_toml::from_reader::<Config, _>(TEXT.as_bytes()).unwrap(),
        config()
    );
}

#[test]
fn writes_what_it_reads() {
    let text = fig_toml::to_string(&config()).unwrap();
    assert_eq!(text, TEXT);
    assert_eq!(fig_toml::to_vec(&config()).unwrap(), TEXT.as_bytes());
    let mut out = Vec::new();
    fig_toml::to_writer(&mut out, &config()).unwrap();
    assert_eq!(out, TEXT.as_bytes());
}

#[test]
fn format_is_the_registered_format() {
    let format = fig_toml::format().unwrap();
    assert_eq!(format.name(), "toml");
    assert_eq!(fig_toml::format().unwrap(), format);
    assert_eq!(fig::Format::by_name("toml"), Some(format));
    assert_eq!(fig_toml::register().unwrap(), format);
}

#[test]
fn a_bad_value_says_where() {
    let e = fig_toml::from_str::<Config>("name = \"fig\"\nport = \"high\"\n").unwrap_err();
    assert_eq!(e.path(), Some("port"));
}

#[test]
fn a_failed_read_is_io() {
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("gone"))
        }
    }
    let e = fig_toml::from_reader::<Config, _>(Broken).unwrap_err();
    assert!(matches!(e, fig::Error::Io(_)), "{e:?}");
}
