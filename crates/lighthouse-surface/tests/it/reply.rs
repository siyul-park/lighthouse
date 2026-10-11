//! Who is calling, and what an operation hands back.

use std::path::PathBuf;

use lighthouse_model::AgentKind;
use lighthouse_surface::{Caller, Context, Reply, reply::Output};
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
    assert!(!ctx.confine, "a person at a terminal is not confined");
}

#[test]
fn context_agent() {
    let ctx = Context::agent("claude");
    assert_eq!(ctx.caller, Caller::Agent("claude".to_owned()));
    assert_eq!(ctx.config, None);
    assert!(ctx.confine);
}

#[test]
fn context_attribution() {
    let root = tempfile::tempdir().unwrap();
    let agent = Context::agent("claude")
        .attribution(root.path(), Some("agent"))
        .unwrap();
    assert_eq!(agent.kind, AgentKind::SoftwareAgent);
    assert_eq!(
        agent.id.as_deref(),
        Some("claude"),
        "the caller is the default"
    );
    let named = Context::agent("claude")
        .attribution(root.path(), Some("agent:other"))
        .unwrap();
    assert_eq!(named.id.as_deref(), Some("other"));
    let error = Context::person(None)
        .attribution(root.path(), Some("robot"))
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
fn reply() {
    let reply = Reply::default();
    assert_eq!((reply.stdout.as_str(), reply.code), ("", 0));
    assert!(reply.notes.is_empty() && reply.value.is_null());
}

#[test]
fn reply_data() {
    let data = json!({"n": 1});
    let json = Reply::data(Output::Json, &data, || unreachable!("no text for json")).unwrap();
    assert_eq!(json.stdout, "{\"n\":1}\n");
    assert_eq!(json.value, data);
    let text = Reply::data(Output::Text, &data, || "one\n".to_owned()).unwrap();
    assert_eq!(text.stdout, "one\n");
    assert!(text.value.is_null());
}

#[test]
fn reply_json() {
    let reply = Reply::json(json!([1, 2])).unwrap();
    assert_eq!(reply.stdout, "[1,2]\n");
    assert_eq!(reply.value, json!([1, 2]));
}

#[test]
fn reply_text() {
    let reply = Reply::text("hello\n".to_owned());
    assert_eq!(reply.stdout, "hello\n");
    assert!(reply.value.is_null());
}

#[test]
fn reply_with_code() {
    let reply = Reply::text("x\n".to_owned()).with_code(3);
    assert_eq!((reply.code, reply.stdout.as_str()), (3, "x\n"));
}

#[test]
fn reply_with_notes() {
    let reply = Reply::text("x\n".to_owned()).with_notes(vec!["note".to_owned()]);
    assert_eq!(reply.notes, ["note"]);
    assert_eq!(reply.stdout, "x\n");
}
