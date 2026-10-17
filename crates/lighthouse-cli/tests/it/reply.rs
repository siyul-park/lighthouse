//! Who is calling, and what an operation hands back.

use std::path::PathBuf;

use lighthouse_cli::surface::{
    Caller, Context, Reply,
    reply::{Body, Format},
};
use lighthouse_model::AgentKind;
use serde_json::json;

#[test]
fn context() {
    assert_eq!(Context::person(None).caller, Caller::Person);
    assert_ne!(Context::agent("claude").caller, Caller::Person);
}

#[test]
fn context_person() {
    let ctx = Context::person(Some(PathBuf::from("lighthouse.toml")));
    assert_eq!(ctx.config, Some(PathBuf::from("lighthouse.toml")));
    assert!(!ctx.confined(), "a person at a terminal is not confined");
}

#[test]
fn context_agent() {
    let ctx = Context::agent("claude");
    assert_eq!(ctx.caller, Caller::Agent("claude".to_owned()));
    assert_eq!(ctx.config, None);
    assert!(ctx.confined());
}

#[test]
fn context_attribution() {
    let agent = Context::agent("claude").attribution(Some("agent")).unwrap();
    assert_eq!(agent.kind, AgentKind::SoftwareAgent);
    assert_eq!(
        agent.id.as_deref(),
        Some("claude"),
        "the caller is the default"
    );
    let named = Context::agent("claude")
        .attribution(Some("agent:other"))
        .unwrap();
    assert_eq!(named.id.as_deref(), Some("other"));
    let error = Context::person(None)
        .attribution(Some("robot"))
        .unwrap_err();
    assert!(error.to_string().contains("is not person or agent"));
}

#[test]
fn context_paths() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "a\n").unwrap();
    let inside = vec![root.path().join("a.txt")];

    let confined = Context::agent("claude")
        .paths(root.path(), &inside)
        .unwrap();
    assert_eq!(confined, vec![inside[0].canonicalize().unwrap()]);
    let error = Context::agent("claude")
        .paths(root.path(), &[outside.path().to_path_buf()])
        .unwrap_err();
    assert!(error.to_string().contains("is outside the project root"));

    let free = vec![outside.path().to_path_buf()];
    assert_eq!(
        Context::person(None).paths(root.path(), &free).unwrap(),
        free
    );
}

#[test]
fn reply_data() {
    let data = json!({"n": 1});
    let json = Reply::data(Format::Json, &data, || unreachable!("no text for json")).unwrap();
    assert_eq!(json.body, Body::Json(data.clone()));
    let text = Reply::data(Format::Text, &data, || "one\n".to_owned()).unwrap();
    assert_eq!(text.body, Body::Text("one\n".to_owned()));
    assert_eq!((text.code, text.notes.len()), (0, 0));
}

#[test]
fn reply_json() {
    let reply = Reply::json(&json!([1, 2])).unwrap();
    assert_eq!(reply.body, Body::Json(json!([1, 2])));
}

#[test]
fn reply_text() {
    let reply = Reply::text("hello\n".to_owned());
    assert_eq!(reply.body, Body::Text("hello\n".to_owned()));
}

#[test]
fn reply_with_code() {
    let reply = Reply::text("x\n".to_owned()).with_code(3);
    assert_eq!(reply.code, 3);
    assert_eq!(reply.body, Body::Text("x\n".to_owned()));
}

#[test]
fn reply_with_notes() {
    let reply = Reply::text("x\n".to_owned()).with_notes(vec!["note".to_owned()]);
    assert_eq!(reply.notes, ["note"]);
    assert_eq!(reply.body, Body::Text("x\n".to_owned()));
}

#[test]
fn reply_with_messages_prefixes_them() {
    let reply = Reply::text(String::new()).with_messages(&["gone".to_owned()]);
    assert_eq!(reply.notes, ["lighthouse: gone"]);
}
