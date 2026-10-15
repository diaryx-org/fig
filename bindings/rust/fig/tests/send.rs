//! A `Document` and an `Editor` move to another thread, are used and freed
//! there: what an async task that holds one across an `.await` needs.

mod common;

use fig::{Document, Editor, Segment};

fn assert_send<T: Send>() {}

#[test]
fn handles_are_send() {
    assert_send::<Document>();
    assert_send::<Editor>();
}

#[test]
fn a_document_and_an_editor_are_used_and_dropped_on_another_thread() {
    let yaml = common::by_name("yaml");
    let doc = Document::parse(b"a: 1\n", yaml).unwrap();
    let mut ed = Editor::open(b"a: 1 # kept\n", yaml).unwrap();
    let (value, source) = std::thread::spawn(move || {
        ed.replace_value(&[Segment::Key("a")], 2i64).unwrap();
        (doc.to_value().unwrap(), ed.source().unwrap().to_owned())
    })
    .join()
    .unwrap();
    assert_eq!(value.get("a").and_then(fig::Value::as_i64), Some(1));
    assert_eq!(source, "a: 2 # kept\n");
}
