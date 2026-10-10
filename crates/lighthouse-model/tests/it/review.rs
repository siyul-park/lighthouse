use lighthouse_model::{
    AgentKind, Attribution, Judgment, Label, Severity, Suppression, SuppressionKind,
    SuppressionStatus, needs_review,
};

#[test]
fn judgment_round_trips_through_text_and_serde() {
    for judgment in Judgment::ALL {
        assert_eq!(judgment.to_string().parse::<Judgment>().unwrap(), *judgment);
        let json = serde_json::to_string(judgment).unwrap();
        assert_eq!(json, format!("\"{judgment}\""));
    }
    assert_eq!(Judgment::NotApplicable.as_str(), "notApplicable");
    let error = "nope".parse::<Judgment>().unwrap_err();
    assert_eq!(
        error.to_string(),
        "unknown judgment `nope` (expected pass, fail, notApplicable)"
    );
}

#[test]
fn a_suppression_is_accepted_unless_it_says_otherwise() {
    let suppression = Suppression::external("accepted-debt");
    assert_eq!(suppression.kind, SuppressionKind::External);
    assert_eq!(suppression.status, SuppressionStatus::Accepted);
    assert!(suppression.in_force());
    let json = serde_json::to_value(&suppression).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "kind": "external", "justification": "accepted-debt" })
    );
    let back: Suppression =
        serde_json::from_value(serde_json::json!({ "kind": "inSource", "justification": "why" }))
            .unwrap();
    assert_eq!(back, Suppression::in_source("why"));
    let proposed: Suppression = serde_json::from_value(serde_json::json!({
        "kind": "external", "status": "underReview", "justification": "later"
    }))
    .unwrap();
    assert!(!proposed.in_force());
}

#[test]
fn agent_kind_reads_the_prov_classes_and_the_old_words() {
    for (text, kind) in [
        ("Person", AgentKind::Person),
        ("human", AgentKind::Person),
        ("SoftwareAgent", AgentKind::SoftwareAgent),
        ("agent", AgentKind::SoftwareAgent),
    ] {
        assert_eq!(text.parse::<AgentKind>().unwrap(), kind, "{text}");
    }
    assert!("robot".parse::<AgentKind>().is_err());
    let attribution = Attribution {
        kind: AgentKind::SoftwareAgent,
        id: Some("claude".to_owned()),
    };
    assert_eq!(
        serde_json::to_value(&attribution).unwrap(),
        serde_json::json!({ "type": "SoftwareAgent", "id": "claude" })
    );
}

#[test]
fn label_follows_the_documented_semantics() {
    assert_eq!(Label::of(Judgment::Fail, false), Label::Positive);
    assert_eq!(Label::of(Judgment::Fail, true), Label::Separate);
    assert_eq!(Label::of(Judgment::Pass, false), Label::Negative);
    assert_eq!(Label::of(Judgment::NotApplicable, false), Label::Negative);
}

#[test]
fn only_a_non_error_finding_nobody_judged_needs_review() {
    assert!(needs_review(Severity::Warn, false));
    assert!(needs_review(Severity::Info, false));
    assert!(!needs_review(Severity::Error, false));
    assert!(!needs_review(Severity::Warn, true));
}
