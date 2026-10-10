use lighthouse_model::{
    AgentKind, Attribution, Diagnostic, Fingerprint, Judgment, Label, Position, Severity, Span,
    Suppression, SuppressionKind, SuppressionStatus,
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

fn finding() -> Diagnostic {
    let at = Position { line: 1, col: 1 };
    let span = Span { start: at, end: at };
    Diagnostic::new(
        "p/a",
        Severity::Error,
        "m",
        "a.go",
        span,
        Fingerprint::from_raw("f"),
    )
}

#[test]
fn diagnostic_suppression_round_trips_and_is_left_out_when_absent() {
    let plain = serde_json::to_value(finding()).unwrap();
    assert!(plain.get("suppression").is_none());
    let mut marked = finding();
    marked.suppression = Some(Suppression::in_source("known"));
    let json = serde_json::to_value(&marked).unwrap();
    assert_eq!(json["suppression"]["kind"], "inSource");
    let back: Diagnostic = serde_json::from_value(json).unwrap();
    assert_eq!(back, marked);
    let old: Diagnostic = serde_json::from_value(plain).unwrap();
    assert_eq!(old.suppression, None);
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
    assert!(Severity::Warn.needs_review(false));
    assert!(Severity::Info.needs_review(false));
    assert!(!Severity::Error.needs_review(false));
    assert!(!Severity::Warn.needs_review(true));
}

#[test]
fn an_unknown_term_names_the_text_and_what_was_expected() {
    let error: lighthouse_model::UnknownTerm = "maybe".parse::<Judgment>().unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("`maybe`") && message.contains("expected"),
        "{message}"
    );
}

#[test]
fn a_person_weighs_more_than_an_agent() {
    assert!(AgentKind::Person.strength() > AgentKind::SoftwareAgent.strength());
}
