use std::{thread, time::Duration};

use lighthouse_model::{AgentKind, Attribution, SignalClass, SignalStatus};
use lighthouse_store::{Error, NewSignal, SignalFilter, Store};
use serde_json::json;

const WEEK: u64 = 7 * 24 * 3600;

fn new(id: &str, class: SignalClass) -> NewSignal {
    NewSignal {
        id: id.to_owned(),
        session: "s1".to_owned(),
        event: "UserPromptSubmit".to_owned(),
        class,
        excerpt: "Never use unwrap.".to_owned(),
        features: json!({ "language": "en" }),
        rule: "en-always-never".to_owned(),
        by: Attribution::new(AgentKind::Person, Some("ana".to_owned())),
    }
}

#[test]
fn a_signal_is_recorded_once_and_read_back_with_its_status() {
    let mut store = Store::open_in_memory().unwrap();
    assert!(
        store
            .add_signal(&new("aaaaaaaaaaaa", SignalClass::Directive), WEEK)
            .unwrap()
    );
    assert!(
        !store
            .add_signal(&new("aaaaaaaaaaaa", SignalClass::Directive), WEEK)
            .unwrap()
    );
    let found = store.signals(&SignalFilter::default()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].status, SignalStatus::Open);
    assert_eq!(found[0].by.id.as_deref(), Some("ana"));

    let status = SignalStatus::Recorded {
        decision: "uid-1".to_owned(),
    };
    store
        .settle_signal("aaaaaaaaaaaa", &status, "agent:claude")
        .unwrap();
    let settled = store.signal("aaaaaaa").unwrap();
    assert_eq!(settled.status, status);
    assert_eq!(settled.settled_by.as_deref(), Some("agent:claude"));
}

#[test]
fn a_prefix_must_be_long_enough_and_unique() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .add_signal(&new("abcdef000001", SignalClass::Directive), WEEK)
        .unwrap();
    store
        .add_signal(&new("abcdef000002", SignalClass::Correction), WEEK)
        .unwrap();
    assert!(matches!(store.signal("abc"), Err(Error::ShortSignal(_))));
    assert!(matches!(
        store.signal("abcdef0"),
        Err(Error::AmbiguousSignal { .. })
    ));
    assert!(matches!(
        store.signal("fffffff"),
        Err(Error::UnknownSignal(_))
    ));
    assert_eq!(
        store.signal("abcdef000002").unwrap().class,
        SignalClass::Correction
    );
}

#[test]
fn filters_pick_the_open_unasked_signals_that_state_a_decision() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .add_signal(&new("000000000001", SignalClass::Directive), WEEK)
        .unwrap();
    store
        .add_signal(&new("000000000002", SignalClass::Abstain), WEEK)
        .unwrap();
    store
        .add_signal(&new("000000000003", SignalClass::Finding), WEEK)
        .unwrap();
    let said = SignalFilter {
        said: true,
        unasked: true,
        ..SignalFilter::default()
    };
    assert_eq!(store.signals(&said).unwrap().len(), 1);
    store.mark_asked(&["000000000001".to_owned()]).unwrap();
    assert!(store.signals(&said).unwrap().is_empty());
}

#[test]
fn writing_purges_the_signals_past_the_retention() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .add_signal(&new("000000000001", SignalClass::Directive), 1)
        .unwrap();
    thread::sleep(Duration::from_millis(1500));
    store
        .add_signal(&new("000000000002", SignalClass::Directive), 1)
        .unwrap();
    let ids: Vec<String> = store
        .signals(&SignalFilter::default())
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, ["000000000002"]);
}

#[test]
fn proposals_keep_what_the_dedupe_check_named() {
    let mut store = Store::open_in_memory().unwrap();
    store.add_proposal("uid-1", "local/a", &[]).unwrap();
    store
        .add_proposal("uid-2", "local/b", &["local/a".to_owned()])
        .unwrap();
    let proposals = store.proposals().unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[1].duplicates, ["local/a"]);
}

fn documents() -> Vec<(String, String)> {
    [
        (
            "design/errors",
            "Errors must be wrapped with context before they cross a module boundary",
        ),
        (
            "design/names",
            "Boolean fields are named with an is or has prefix",
        ),
        ("design/ko", "에러는 항상 Result로 반환해야 한다"),
    ]
    .map(|(id, text)| (id.to_owned(), text.to_owned()))
    .to_vec()
}

#[test]
fn fts5_ranks_the_document_that_shares_the_most_text_first() {
    let store = Store::open_in_memory().unwrap();
    let found = store
        .rank(&documents(), "errors wrapped with context", 3)
        .unwrap();
    assert_eq!(found[0].id, "design/errors");
    assert!(found[0].similarity > 0.5 && found[0].similarity <= 1.0);
    assert_eq!(
        store
            .rank(&documents(), "errors wrapped with context", 3)
            .unwrap(),
        found
    );
}

#[test]
fn fts5_matches_hangul_through_trigrams_and_nothing_for_a_query_without_shared_text() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(
        store.rank(&documents(), "에러는 항상 반환", 3).unwrap()[0].id,
        "design/ko"
    );
    assert!(
        store
            .rank(&documents(), "quantum entanglement", 3)
            .unwrap()
            .is_empty()
    );
    assert!(store.rank(&documents(), "ab", 3).unwrap().is_empty());
    assert!(store.rank(&[], "errors", 3).unwrap().is_empty());
}
