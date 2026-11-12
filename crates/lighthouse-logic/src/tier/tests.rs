use std::collections::BTreeMap;

use lighthouse_spec::{Catalog, Env};

use super::{Tier, tier};

const EXAMPLES: &str = "  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: a.txt, body: x }]
      expect: [{ line: 1 }]
FIXED    - name: good
      language: text
      kind: valid
      files: [{ path: a.txt, body: x }]
";

/// The tier of a decision whose `check` and `fix` blocks are `check` and `fix`.
fn classify(check: &str, fix: &str) -> (Tier, String) {
    classify_in("symbol", check, fix)
}

fn classify_in(subject: &str, check: &str, fix: &str) -> (Tier, String) {
    let fixed = if fix.is_empty() {
        ""
    } else {
        "      fixed: [{ path: a.txt, body: y }]\n"
    };
    let severity = if check.is_empty() {
        ""
    } else {
        "  severity: warn\n"
    };
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: {subject} }}\n  requirement: A probe MUST hold.\n{severity}{check}{fix}{}",
        EXAMPLES.replace("FIXED", fixed)
    );
    let env = Env::standard().unwrap();
    let catalog = Catalog::from_local(BTreeMap::from([("probe.yaml".to_owned(), text)]), &env)
        .unwrap_or_else(|e| panic!("{e}"));
    let decision = catalog.decision("local/probe").unwrap();
    let found = tier(&env, decision);
    (found.tier, found.tier_reason)
}

fn cel(condition: &str) -> String {
    format!(
        "  check:\n    type: cel\n    select: symbol\n    where: {}\n    message: m\n",
        serde_json::to_string(condition).unwrap()
    )
}

fn fix(edits: &str) -> String {
    format!(
        "  fix:\n    safety: unsafe\n    type: cel\n    edits: {}\n",
        serde_json::to_string(edits).unwrap()
    )
}

#[test]
fn a_decision_without_a_check_or_with_a_model_or_a_command_is_text() {
    let model = "  check:\n    type: model\n    prompt: p\n";
    let command = "  check:\n    type: command\n    files: [\"**/*.md\"]\n    command: [echo, \"{file}\"]\n    output: status\n    success: [0]\n";

    assert_eq!(classify("", ""), (Tier::T, "no-check".to_owned()));
    assert_eq!(classify(model, ""), (Tier::T, "model".to_owned()));
    assert_eq!(classify_in("file", command, "").1, "command");
}

#[test]
fn a_where_of_false_without_abstain_is_no_condition() {
    assert_eq!(
        classify(&cel("false"), ""),
        (Tier::T, "no-condition".to_owned())
    );
}

#[test]
fn a_check_that_abstains_is_text() {
    let abstain = format!("{}    abstain: 'true'\n", cel("symbol.n > 1"));
    assert_eq!(classify(&abstain, "").1, "abstain");
}

#[test]
fn a_check_of_the_fragment_is_formal() {
    assert_eq!(
        classify(&cel("symbol.n > 1"), ""),
        (Tier::F, "translated".to_owned())
    );
}

#[test]
fn a_fix_that_moves_or_reorders_makes_a_decision_an_order() {
    let moves = fix("[move(symbol, after(symbol))]");
    let reorders = fix("[reorder([symbol])]");
    let removes = fix("[remove(symbol)]");

    assert_eq!(
        classify(&cel("symbol.n > 1"), &moves),
        (Tier::O, "fix:move".to_owned())
    );
    assert_eq!(classify(&cel("symbol.n > 1"), &reorders).1, "fix:reorder");
    assert_eq!(classify(&cel("symbol.n > 1"), &removes).0, Tier::F);
}

#[test]
fn a_move_to_the_edge_of_a_file_is_placement() {
    let across = fix("[move(symbol, end(symbol.file))]");

    assert_eq!(
        classify(&cel("symbol.n > 1"), &across),
        (Tier::T, "placement".to_owned())
    );
    assert_eq!(
        classify(&cel("symbol.n > 1"), &fix("[move(symbol, before(symbol))]")).0,
        Tier::O
    );
}

#[test]
fn what_leaves_the_fragment_names_the_construct() {
    assert_eq!(
        classify(&cel("symbol.a > symbol.b"), ""),
        (Tier::T, "outside-fragment:two-fields".to_owned())
    );
}

#[test]
fn as_str_names_a_tier_by_its_letter() {
    assert_eq!(
        [Tier::F, Tier::O, Tier::T].map(Tier::as_str),
        ["F", "O", "T"]
    );
}
