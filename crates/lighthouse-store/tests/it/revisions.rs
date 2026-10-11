use lighthouse_model::{AgentKind, Attribution};
use lighthouse_store::{Change, Records, RevisionSpec, Store, append_revision};
use serde_json::json;

fn revision(decision: &str, model: Option<&str>) -> RevisionSpec {
    RevisionSpec {
        decision: decision.to_owned(),
        parent: None,
        meaning_version: Some("m1".to_owned()),
        check_revision: Some("c1".to_owned()),
        change: Change::Minor,
        evaluation: json!({ "precision": 0.95, "n": 40 }),
        approval: Attribution::new(AgentKind::Person, Some("kim".to_owned())),
        model: model.map(str::to_owned),
        generated_at_time: "2026-10-11T00:00:00Z".to_owned(),
    }
}

#[test]
fn a_project_without_a_log_has_no_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let records = Records::read(dir.path()).unwrap();
    assert!(records.revisions.is_empty() && records.attached().is_empty());
}

#[test]
fn the_latest_revision_of_a_decision_decides_the_model_it_is_bound_to() {
    let dir = tempfile::tempdir().unwrap();
    append_revision(dir.path(), revision("uid-a", Some("local/learned@1"))).unwrap();
    append_revision(dir.path(), revision("uid-b", Some("openai/gpt"))).unwrap();
    append_revision(dir.path(), revision("uid-a", Some("local/learned@2"))).unwrap();
    let attached = Records::read(dir.path()).unwrap().attached();
    assert_eq!(attached["uid-a"], "local/learned@2");
    assert_eq!(attached["uid-b"], "openai/gpt");

    append_revision(dir.path(), revision("uid-a", None)).unwrap();
    let attached = Records::read(dir.path()).unwrap().attached();
    assert!(!attached.contains_key("uid-a"));
}

#[test]
fn a_revision_is_read_back_whole_and_the_store_still_opens_over_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let id = append_revision(dir.path(), revision("uid-a", Some("openai/gpt"))).unwrap();
    let records = Records::read(dir.path()).unwrap();
    assert_eq!(records.revisions.len(), 1);
    assert_eq!(records.revisions[0].id, id);
    assert_eq!(
        records.revisions[0].spec,
        revision("uid-a", Some("openai/gpt"))
    );
    assert!(Store::open(dir.path()).is_ok());
}
