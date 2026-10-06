use std::{env, fs, path::Path};

const CHECKED_IN: &str = "../../protocol/schema/lighthouse-protocol-0.1.json";

fn rendered() -> String {
    let mut text = serde_json::to_string_pretty(&lighthouse_protocol::schema()).unwrap();
    text.push('\n');
    text
}

/// The checked-in schema is what external SDKs generate their types from, so
/// it must track the Rust wire types. `UPDATE_SCHEMA=1 cargo test -p
/// lighthouse-protocol` rewrites it.
#[test]
fn checked_in_schema_matches_the_wire_types() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CHECKED_IN);
    if env::var_os("UPDATE_SCHEMA").is_some() {
        fs::write(&path, rendered()).unwrap();
        return;
    }
    let stored = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        stored == rendered(),
        "{} is stale; run `UPDATE_SCHEMA=1 cargo test -p lighthouse-protocol`",
        path.display()
    );
}

#[test]
fn schema_covers_every_method_and_the_version_is_pinned() {
    let schema = lighthouse_protocol::schema();
    let methods = &schema["properties"];
    assert!(methods["initialize"].is_object() && methods["index"].is_object());
    assert_eq!(lighthouse_protocol::VERSION, "0.1");
    assert!(CHECKED_IN.ends_with("lighthouse-protocol-0.1.json"));
}
