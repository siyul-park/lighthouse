use lighthouse_config::Format;
use lighthouse_session::migrate::plugin::{is_legacy, migrate};

#[test]
fn a_manifest_from_before_the_resource_model_migrates() {
    let old: serde_json::Value =
        serde_json::from_str(r#"{"id":"x","version":"1","command":"./run","args":["-v"]}"#)
            .unwrap();

    let document = migrate(&old).unwrap();

    let parsed = lighthouse_rpc::parse(Format::Json, "x", &document.to_string()).unwrap();
    assert_eq!(
        (parsed.id.as_str(), parsed.command.as_str()),
        ("x", "./run")
    );
    assert_eq!(parsed.args, ["-v"]);
    let legacy = lighthouse_rpc::parse(Format::Json, "x", &old.to_string());
    assert!(legacy.unwrap_err().contains("lighthouse spec migrate"));
}

#[test]
fn is_legacy_recognizes_an_old_manifest() {
    assert!(is_legacy(
        &serde_json::json!({ "id": "x", "version": "1", "command": "./run" })
    ));
    assert!(!is_legacy(&serde_json::json!({ "id": "x" })));
}
