use lighthouse_engine::RuleTester;
use lighthouse_spec::Catalog;

#[test]
fn every_implemented_pattern_passes_its_catalog_examples() {
    let failures = RuleTester::new(lighthouse_builtin::registry, Catalog::bundled()).check_all();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
