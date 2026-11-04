use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use std::path::Path;

use lighthouse_plugin::{Memo, Pending};

#[derive(Default)]
struct Counter(AtomicUsize);

#[derive(Default)]
struct Other(AtomicUsize);

#[test]
fn memo() {
    let memo = Memo::default();

    let first = memo.slot::<Counter>();
    first.0.fetch_add(1, Ordering::SeqCst);

    assert_eq!(memo.slot::<Counter>().0.load(Ordering::SeqCst), 1);
    assert_eq!(memo.slot::<Other>().0.load(Ordering::SeqCst), 0);
}

#[test]
fn memo_slot() {
    let memo = Memo::default();

    let slots: Vec<Arc<Counter>> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| memo.slot::<Counter>()))
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).collect()
    });

    assert!(slots.iter().all(|slot| Arc::ptr_eq(slot, &slots[0])));
}

#[test]
fn pending() {
    let pending = Pending::default();
    assert!(
        pending.wants("r", Path::new("a")),
        "nothing looked up: wanted"
    );

    pending.add("r", Path::new("a"));

    assert!(pending.wants("r", Path::new("a")));
    assert!(!pending.wants("r", Path::new("b")));
    assert!(pending.wants("other", Path::new("b")));
}

#[test]
fn pending_add() {
    let pending = Pending::default();
    pending.add("r", Path::new("a"));
    pending.add("r", Path::new("b"));

    assert!(pending.wants("r", Path::new("a")) && pending.wants("r", Path::new("b")));
}

#[test]
fn pending_wants() {
    let pending = Pending::default();
    pending.add("r", Path::new("a"));

    assert!(pending.wants("r", Path::new("a")));
    assert!(!pending.wants("r", Path::new("c")));
}

#[test]
fn annotations_of() {
    let annotations = lighthouse_plugin::Annotations::default();
    assert!(annotations.of(std::path::Path::new("a.rs")).is_none());

    annotations.insert("a.rs".into(), vec![serde_json::json!({ "rule": "x/y" })]);
    annotations.insert("b.rs".into(), Vec::new());

    assert_eq!(
        annotations.of(std::path::Path::new("a.rs")).unwrap().len(),
        1
    );
    assert!(
        annotations
            .of(std::path::Path::new("b.rs"))
            .unwrap()
            .is_empty()
    );
}
