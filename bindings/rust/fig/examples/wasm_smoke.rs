//! Links and runs fig on a wasm target: a JSON document read and written
//! back as YAML, through the core and two languages' libraries.
//!
//!   cargo run --example wasm_smoke --target wasm32-wasip1   # under wasmtime
fn main() {
    let json = fig_json::register().unwrap();
    let yaml = fig_yaml::register().unwrap();
    let doc = fig::Document::parse(br#"{"name":"fig"}"#, json).unwrap();
    assert_eq!(doc.serialize(yaml).unwrap(), "name: fig\n");
    println!("ok");
}
