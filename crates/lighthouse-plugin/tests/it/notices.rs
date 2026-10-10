use std::thread;

use lighthouse_plugin::Notices;

#[test]
fn notices() {
    let notices = Notices::default();
    assert!(notices.take().is_empty());

    notices.push("first");
    notices.push(String::from("second"));

    assert_eq!(notices.take(), ["first", "second"]);
    assert!(notices.take().is_empty(), "taking empties it");
}

#[test]
fn notices_take_keeps_what_rules_running_together_push() {
    let notices = Notices::default();
    thread::scope(|scope| {
        for n in 0..8 {
            let notices = &notices;
            scope.spawn(move || notices.push(format!("rule {n}")));
        }
    });
    assert_eq!(notices.take().len(), 8);
}
