use lighthouse_model::SignalStatus;

fn every() -> [SignalStatus; 4] {
    [
        SignalStatus::Open,
        SignalStatus::Recorded {
            decision: "uid-1".to_owned(),
        },
        SignalStatus::Linked {
            decision: "uid-2".to_owned(),
        },
        SignalStatus::Dismissed {
            reason: "a request".to_owned(),
        },
    ]
}

#[test]
fn signal_status_name() {
    let statuses = every();
    let names: Vec<&str> = statuses.iter().map(SignalStatus::name).collect();
    assert_eq!(names, ["open", "recorded", "linked", "dismissed"]);
    // The name is the tag of the status in JSON.
    for status in every() {
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["status"], status.name());
    }
}

#[test]
fn signal_status_detail() {
    let statuses = every();
    let details: Vec<Option<&str>> = statuses.iter().map(SignalStatus::detail).collect();
    assert_eq!(
        details,
        [None, Some("uid-1"), Some("uid-2"), Some("a request")]
    );
}

#[test]
fn signal_status_from_parts() {
    // What the store keeps, a name and a detail, gives the status back.
    for status in every() {
        let detail = status.detail().map(str::to_owned);
        assert_eq!(
            SignalStatus::from_parts(status.name(), detail),
            Some(status)
        );
    }
    assert_eq!(
        SignalStatus::from_parts("open", Some("ignored".to_owned())),
        Some(SignalStatus::Open)
    );
    assert_eq!(SignalStatus::from_parts("settled", None), None);
}
