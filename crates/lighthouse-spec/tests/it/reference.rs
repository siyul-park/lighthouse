#[test]
fn a_decision_reference_carries_its_requirement_and_examples() {
    let decision = lighthouse_test_support::bundled()
        .decision("design/prefer-method")
        .unwrap();
    let told = decision.reference();

    assert_eq!(told.id, "design/prefer-method");
    assert_eq!(told.prompt, decision.requirement);
    assert!(!told.invalid.is_empty());
    assert!(!told.valid.is_empty());
    assert!(
        told.invalid
            .iter()
            .chain(&told.valid)
            .all(|s| !s.code.is_empty())
    );
}
