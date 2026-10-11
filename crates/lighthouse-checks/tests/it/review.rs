use lighthouse_checks::decision_ref;
use lighthouse_plugin::ResultKind;
use lighthouse_spec::Catalog;

#[test]
fn a_decision_is_told_to_a_model_by_its_requirement_and_examples() {
    let decision = Catalog::bundled().decision("design/feature-envy").unwrap();
    let told = decision_ref(decision);

    assert_eq!(told.id, "design/feature-envy");
    assert_eq!(told.prompt, decision.requirement);
    assert!(told.examples.iter().any(|s| s.kind == ResultKind::Fail));
    assert!(told.examples.iter().any(|s| s.kind == ResultKind::Pass));
    assert!(told.examples.iter().all(|s| !s.code.is_empty()));
}
