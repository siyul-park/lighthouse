//! The tier of every decision of the catalog: the coverage report of the
//! formal engines, as a snapshot a change of tier must review.

use std::collections::BTreeMap;

use lighthouse_logic::{Classified, Tier, tier};

fn classified() -> BTreeMap<String, Classified> {
    let env = lighthouse_test_support::cel();
    lighthouse_test_support::full()
        .decisions()
        .map(|decision| (decision.id().to_owned(), tier(&env, decision)))
        .collect()
}

fn with(tiers: &BTreeMap<String, Classified>, wanted: Tier) -> Vec<&str> {
    tiers
        .iter()
        .filter(|(_, c)| c.tier == wanted)
        .map(|(id, _)| id.as_str())
        .collect()
}

#[test]
fn the_coverage_of_the_catalog_by_tier() {
    let tiers = classified();
    let count = |t| with(&tiers, t).len();
    let mut report = format!(
        "{} decisions: F {}, O {}, T {}\n\n",
        tiers.len(),
        count(Tier::F),
        count(Tier::O),
        count(Tier::T)
    );
    for (id, c) in &tiers {
        report.push_str(&format!("{id}\t{}\t{}\n", c.tier.as_str(), c.tier_reason));
    }

    insta::assert_snapshot!("coverage", report);
}

#[test]
fn the_order_decisions_are_those_whose_fix_moves_declarations() {
    let tiers = classified();

    assert_eq!(
        with(&tiers, Tier::O),
        [
            "design/declaration-groups",
            "design/top-down-order",
            "design/vertical-distance",
            "testing/file-layout",
        ]
    );
}

#[test]
fn the_owner_file_decisions_name_the_construct_that_leaves_the_fragment() {
    let tiers = classified();

    assert_eq!(
        tiers["design/owner-file"].tier_reason,
        "outside-fragment:computed-list"
    );
    assert_eq!(
        tiers["testing/owner-file"].tier_reason,
        "outside-fragment:index"
    );
    assert!(tiers.values().all(|c| c.tier_reason != "placement"));
}

#[test]
fn a_decision_the_translator_reads_is_formal_and_model_decisions_are_text() {
    let tiers = classified();

    assert_eq!(tiers["core/max-lines"].tier, Tier::F);
    assert_eq!(tiers["design/no-unused-symbol"].tier, Tier::F);
    assert_eq!(tiers["core/formatted"].tier_reason, "command");
    assert!(tiers.values().any(|c| c.tier_reason == "model"));
}
