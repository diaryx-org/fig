//! Scientific-notation float text (`1.0e300`) must be read back as the same
//! float by every format that fig writes it to — not as a string or an int.

#[test]
fn float_scientific_text_survives_every_format() {
    for f in [1e300f64, 1e-7, -2.5e-9, 1.0, 0.1] {
        for name in ["json", "yaml", "toml", "fig"] {
            let fmt = by_name(name);
            let v = fig::Value::Map(vec![(fig::Value::Str("k".into()), fig::Value::Float(f))]);
            let text = v.serialize(fmt).unwrap();
            let doc = fig::Document::parse(text.as_bytes(), fmt).unwrap();
            match doc.to_value().unwrap() {
                fig::Value::Map(entries) => match &entries[0].1 {
                    fig::Value::Float(g) => {
                        assert_eq!(g.to_bits(), f.to_bits(), "{name}: {text:?}")
                    }
                    other => panic!("{name}: {text:?} read back as {other:?}"),
                },
                other => panic!("{name}: {other:?}"),
            }
        }
    }
}

/// The registered format named `name`; a test that names one this build
/// does not carry is a test bug.
#[allow(dead_code)]
fn by_name(name: &str) -> fig::Format {
    fig::Format::by_name(name).unwrap_or_else(|| panic!("no format named {name:?}"))
}
