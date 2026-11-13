//! The fields of a decision document that the engine works out are asked for
//! only by the checks that read them.

use lighthouse_test_support::registry;

#[test]
fn only_a_check_that_reads_conflicts_asks_for_them() {
    let registry = registry();

    let reading = registry.rule("core/no-contradictory-decisions").unwrap();
    let naming = registry.rule("core/decision-naming").unwrap();
    let code = registry.rule("core/max-lines").unwrap();

    assert_eq!(reading.reads_documents(), ["conflicts"]);
    assert!(naming.reads_documents().is_empty());
    assert!(code.reads_documents().is_empty());
}
