use lighthouse_checks::decision_ref;
use lighthouse_spec::Catalog;

#[test]
fn a_decision_is_told_to_a_model_by_its_requirement_and_examples() {
    let decision = Catalog::bundled().decision("design/feature-envy").unwrap();
    let told = decision_ref(decision);

    assert_eq!(told.id, "design/feature-envy");
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
