//! Every core `ExtKind` reaches the wrapper. A kind the wrapper does not name
//! is one `Document::to_value` silently reads as `None` from
//! `fig_node_extended` — which is what happened to the two plist kinds for a
//! release, since nothing held the three mirrors of `FigExtKind` together.
//!
//! Run with `cargo test -p fig --features plist`: the two kinds are plist's.
#![cfg(feature = "plist")]

use fig::{Document, ExtKind, Format, Value};

#[test]
fn plist_date_and_data_reach_the_wrapper() {
    let src = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>when</key><date>2026-09-10T12:00:00Z</date>
  <key>blob</key><data>aGVsbG8=</data>
</dict>
</plist>
"#;
    let doc = Document::parse(src, Format::Plist).expect("plist parses");
    let value = doc.to_value().expect("to_value");
    let map = value.as_mapping().expect("root is a dict");
    let get = |k: &str| map.iter().find(|(key, _)| key.as_str() == Some(k)).map(|(_, v)| v).unwrap();
    assert_eq!(
        get("when"),
        &Value::Extended { kind: ExtKind::PlistDate, text: "2026-09-10T12:00:00Z".into() }
    );
    assert_eq!(get("blob"), &Value::Extended { kind: ExtKind::PlistData, text: "aGVsbG8=".into() });
}
