fn main() {
    let _ = fig::Document::parse(br#"{"name":"fig"}"#, fig::Format::by_name("json").unwrap());
}
