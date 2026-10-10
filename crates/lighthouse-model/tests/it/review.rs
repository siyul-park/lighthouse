use lighthouse_model::{Label, Reason, ReviewerKind, Verdict};

#[test]
fn verdict_accepts_only_its_own_reasons() {
    assert!(Verdict::Confirmed.validate(Reason::Fixed).is_ok());
    assert!(Verdict::Confirmed.validate(Reason::Unspecified).is_ok());
    assert!(Verdict::Rejected.validate(Reason::FalsePositive).is_ok());
    assert!(Verdict::Deferred.validate(Reason::Unspecified).is_ok());

    assert!(Verdict::Rejected.validate(Reason::Unspecified).is_err());
    assert!(Verdict::Rejected.validate(Reason::Fixed).is_err());
    assert!(Verdict::Confirmed.validate(Reason::FalsePositive).is_err());
    let error = Verdict::Deferred.validate(Reason::Fixed).unwrap_err();
    assert_eq!(
        error.to_string(),
        "reason `fixed` does not fit verdict `deferred` (expected none)"
    );
}

#[test]
fn reason_round_trips_through_text_and_serde() {
    for reason in Reason::ALL {
        assert_eq!(reason.to_string().parse::<Reason>().unwrap(), *reason);
        let json = serde_json::to_string(reason).unwrap();
        assert_eq!(json, format!("\"{reason}\""));
    }
    let error = "nope".parse::<Reason>().unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("unknown reason `nope` (expected fixed, ")
    );
}

#[test]
fn reviewer_kind_parses_agent_and_human() {
    assert_eq!(
        "agent".parse::<ReviewerKind>().unwrap(),
        ReviewerKind::Agent
    );
    assert_eq!(
        "human".parse::<ReviewerKind>().unwrap(),
        ReviewerKind::Human
    );
    assert!("robot".parse::<ReviewerKind>().is_err());
}

#[test]
fn label_follows_the_documented_semantics() {
    let label = |verdict, reason| Label::of(verdict, reason);
    assert_eq!(label(Verdict::Confirmed, Reason::Fixed), Label::Positive);
    assert_eq!(
        label(Verdict::Confirmed, Reason::AcceptedDebt),
        Label::Positive
    );
    assert_eq!(
        label(Verdict::Rejected, Reason::FalsePositive),
        Label::Negative
    );
    assert_eq!(
        label(Verdict::Rejected, Reason::ScopeTooBroad),
        Label::Negative
    );
    for reason in [
        Reason::IntentionalException,
        Reason::ProjectAllowed,
        Reason::NotWorthFixing,
    ] {
        assert_eq!(label(Verdict::Rejected, reason), Label::Separate);
    }
    assert_eq!(
        label(Verdict::Deferred, Reason::Unspecified),
        Label::Unlabeled
    );
}

#[test]
fn reasons_lists_what_each_verdict_may_carry() {
    assert_eq!(Verdict::Deferred.reasons(), [Reason::Unspecified]);
    assert!(Verdict::Rejected.reasons().len() == 5);
    assert!(!Verdict::Rejected.reasons().contains(&Reason::Unspecified));
}
