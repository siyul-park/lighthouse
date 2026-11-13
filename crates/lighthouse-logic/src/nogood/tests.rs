use std::collections::BTreeMap;

use lighthouse_spec::{Catalog, Decision, Env as Cel};
use serde_json::{Map, json};

use super::{
    Assumption, Env, Label, MAX_DECISIONS, Nogoods, Unsolved, digest, hitting_sets, is_hitting,
    is_minimal_hitting, is_resolved,
};

fn assumption(name: &str) -> Assumption {
    Assumption {
        uid: name.to_owned(),
        digest: "1".to_owned(),
    }
}

fn env(names: &[&str]) -> Env {
    Env::new(names.iter().map(|n| assumption(n)))
}

fn label(envs: &[&[&str]]) -> Label {
    Label::new(envs.iter().map(|e| env(e)))
}

fn names(label: &Label) -> Vec<Vec<String>> {
    label
        .iter()
        .map(|e| e.iter().map(|a| a.uid.clone()).collect())
        .collect()
}

/// Every order of `items`.
fn orders<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    (0..items.len())
        .flat_map(|at| {
            let mut rest = items.to_vec();
            let first = rest.remove(at);
            orders(&rest).into_iter().map(move |mut tail| {
                tail.insert(0, first.clone());
                tail
            })
        })
        .collect()
}

#[test]
fn an_environment_is_a_sorted_set_of_assumptions() {
    let e = env(&["b", "a", "b"]);

    assert_eq!(e.len(), 2);
    assert!(!e.is_empty() && Env::default().is_empty());
    assert_eq!(
        e.iter().map(|a| a.uid.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(e.has(&assumption("a")) && !e.has(&assumption("c")));
    assert!(env(&["a"]).is_subset(&e) && !e.is_subset(&env(&["a"])));
    assert_eq!(env(&["a"]).union(&env(&["c"])), env(&["a", "c"]));
    assert_eq!(e.without(&assumption("a")), env(&["b"]));
}

/// The decision `local/probe` with this requirement and condition.
fn decision(requirement: &str, condition: &str) -> Decision {
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: symbol }}\n  requirement: {requirement}\n  severity: warn\n  options:\n    properties:\n      max:\n        type: integer\n        default: 3\n        description: A bound.\n  check:\n    type: cel\n    select: symbol\n    where: {condition}\n    message: m\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: x }}]\n"
    );
    let cel = Cel::standard().unwrap();
    let catalog =
        Catalog::from_local(BTreeMap::from([("probe.yaml".to_owned(), text)]), &cel).unwrap();
    catalog.decision("local/probe").unwrap().clone()
}

#[test]
fn what_changes_the_flagging_makes_another_assumption() {
    let base = decision("A probe MUST hold.", "symbol.n > 1");
    let same = Assumption::of(&base, &options(4), None).unwrap();

    assert_eq!(same, Assumption::of(&base, &options(4), None).unwrap());
    assert_eq!(same.uid, "local/probe");
    assert_eq!(same.digest, digest(&base, &options(4), None).unwrap());
    let other_options = Assumption::of(&base, &options(5), None).unwrap();
    assert_ne!(same, other_options, "the options");
    let other_check = decision("A probe MUST hold.", "symbol.n > 2");
    assert_ne!(
        same,
        Assumption::of(&other_check, &options(4), None).unwrap(),
        "the check"
    );
    let other_meaning = decision("A probe MUST NOT hold.", "symbol.n > 1");
    assert_ne!(
        same,
        Assumption::of(&other_meaning, &options(4), None).unwrap(),
        "the meaning"
    );
    assert_eq!(
        Assumption::of(&base, &Map::new(), None).unwrap().uid,
        Assumption::of(&other_check, &Map::new(), None).unwrap().uid
    );
}

#[test]
fn a_default_spelled_out_is_the_same_identity_as_one_left_out() {
    let base = decision("A probe MUST hold.", "symbol.n > 1");
    let left_out = digest(&base, &Map::new(), None).unwrap();

    assert_eq!(left_out, digest(&base, &options(3), None).unwrap());
    assert_ne!(left_out, digest(&base, &options(4), None).unwrap());
}

#[test]
fn an_option_the_decision_does_not_declare_is_an_error() {
    let base = decision("A probe MUST hold.", "symbol.n > 1");
    let unknown = json!({ "min": 1 }).as_object().cloned().unwrap();

    assert!(digest(&base, &unknown, None).is_err());
    assert!(Assumption::of(&base, &unknown, None).is_err());
}

/// The configured options `{ max: value }`.
fn options(value: i64) -> Map<String, serde_json::Value> {
    json!({ "max": value }).as_object().cloned().unwrap()
}

#[test]
fn an_environment_and_an_assumption_round_trip_through_json() {
    let e = env(&["a", "b"]);

    let back: Env = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();

    assert_eq!(back, e);
}

#[test]
fn a_label_keeps_only_minimal_environments() {
    let kept = label(&[&["a", "b"], &["a"], &["b", "c"], &["a", "c"]]);

    assert_eq!(names(&kept), [vec!["a"], vec!["b", "c"]]);
    assert_eq!(kept.len(), 2);
    assert!(!kept.is_empty());
    assert!(kept.contains(&env(&["a"])) && !kept.contains(&env(&["a", "b"])));
    assert!(kept.covers(&env(&["a", "z"])) && !kept.covers(&env(&["b"])));
}

#[test]
fn insert_drops_the_supersets_and_refuses_a_superset() {
    let mut label = Label::default();

    assert!(label.insert(env(&["a", "b"])));
    assert!(label.insert(env(&["a", "c"])));
    assert!(label.insert(env(&["a"])));
    assert_eq!(names(&label), [vec!["a"]]);
    assert!(!label.insert(env(&["a", "d"])));
    assert!(!label.insert(env(&["a"])));
    assert_eq!(names(&label), [vec!["a"]]);
}

#[test]
fn the_order_of_insertion_does_not_change_the_label_or_the_nogoods() {
    let table: [&[&[&str]]; 3] = [
        &[&["a", "b"], &["a"], &["b", "c"], &["c"]],
        &[&["a", "b", "c"], &["a", "b"], &["b", "c"], &["a", "c"]],
        &[&["x"], &["y", "z"], &["x", "y"], &["y"], &["z"]],
    ];
    for envs in table {
        let all: Vec<Env> = envs.iter().map(|e| env(e)).collect();
        let reference = Label::new(all.clone());
        for order in orders(&all) {
            let mut nogoods = Nogoods::default();
            for e in &order {
                nogoods.insert(e.clone());
            }
            assert_eq!(Label::new(order.clone()), reference, "{order:?}");
            assert_eq!(
                nogoods.iter().collect::<Vec<_>>(),
                reference.iter().collect::<Vec<_>>(),
                "{order:?}"
            );
        }
    }
}

#[test]
fn a_nogood_of_one_decision_rules_out_every_environment_with_it() {
    let mut nogoods = Nogoods::default();
    assert!(nogoods.is_empty());
    nogoods.insert(env(&["a", "b"]));

    assert!(nogoods.insert(env(&["a"])));
    assert!(!nogoods.insert(env(&["a", "c"])));
    assert!(nogoods.contains(&env(&["a"])) && !nogoods.contains(&env(&["a", "c"])));
    assert!(nogoods.rules_out(&env(&["a", "c", "d"])));
    assert!(!nogoods.rules_out(&env(&["b", "c"])));
    assert_eq!(nogoods.len(), 1);
    assert_eq!(nogoods.iter().count(), 1);
}

#[test]
#[cfg(not(debug_assertions))]
fn the_empty_environment_is_never_a_nogood() {
    let mut nogoods = Nogoods::default();

    assert!(!nogoods.insert(Env::default()));
    assert!(nogoods.is_empty() && !nogoods.rules_out(&env(&["z"])));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "the empty environment is no nogood")]
fn the_empty_environment_is_never_a_nogood() {
    Nogoods::default().insert(Env::default());
}

#[test]
fn the_product_of_two_labels_holds_when_both_data_hold() {
    let flagged_a = label(&[&["x"], &["y"]]);
    let flagged_b = label(&[&["y"], &["z"]]);

    let both = flagged_a.product(&flagged_b);

    assert_eq!(names(&both), [vec!["x", "z"], vec!["y"]]);
    assert_eq!(both, flagged_b.product(&flagged_a));
}

#[test]
fn is_hitting_asks_that_every_arrangement_is_flagged() {
    let flagged = [env(&["x", "y"]), env(&["y", "z"])];

    assert!(is_hitting(&env(&["y"]), &flagged));
    assert!(is_hitting(&env(&["x", "z"]), &flagged));
    assert!(!is_hitting(&env(&["x"]), &flagged));
    assert!(is_hitting(&Env::default(), &[]));
    assert!(!is_hitting(&env(&["x"]), &[Env::default()]));
}

#[test]
fn is_minimal_hitting_asks_that_no_decision_can_be_removed() {
    let flagged = [env(&["x", "y"]), env(&["y", "z"])];

    assert!(is_minimal_hitting(&env(&["y"]), &flagged));
    assert!(is_minimal_hitting(&env(&["x", "z"]), &flagged));
    assert!(!is_minimal_hitting(&env(&["x", "y"]), &flagged));
    assert!(!is_minimal_hitting(&env(&["x"]), &flagged));
}

/// Every minimal hitting set, from all the subsets of the decisions.
fn brute_force(flagged: &[Env], universe: &[&str]) -> Label {
    let mut found = Label::default();
    for mask in 0..1u32 << universe.len() {
        let chosen = Env::new(
            universe
                .iter()
                .enumerate()
                .filter(|(at, _)| mask >> at & 1 == 1)
                .map(|(_, n)| assumption(n)),
        );
        if is_minimal_hitting(&chosen, flagged) {
            found.insert(chosen);
        }
    }
    found
}

#[test]
fn hitting_sets_are_the_minimal_hitting_sets_of_brute_force() {
    let universe = ["a", "b", "c", "d", "e", "f", "g"];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % n as u64).unwrap()
    };
    for _ in 0..500 {
        let arrangements = 1 + next(6);
        let flagged: Vec<Env> = (0..arrangements)
            .map(|_| {
                let size = 1 + next(4);
                Env::new((0..size).map(|_| assumption(universe[next(universe.len())])))
            })
            .collect();

        let found = hitting_sets(&flagged).unwrap();

        assert_eq!(found, brute_force(&flagged, &universe), "{flagged:?}");
        assert!(found.iter().all(|e| is_minimal_hitting(e, &flagged)));
    }
}

#[test]
fn hitting_sets_of_a_chain_choose_one_decision_for_each_arrangement() {
    let flagged = [env(&["x", "y"]), env(&["y", "z"])];

    assert_eq!(
        names(&hitting_sets(&flagged).unwrap()),
        [vec!["x", "z"], vec!["y"]]
    );
}

#[test]
fn no_arrangement_is_an_error_and_an_unflagged_one_leaves_no_set() {
    assert_eq!(hitting_sets(&[]), Err(Unsolved::NoArrangement));
    assert!(
        hitting_sets(&[env(&["x"]), Env::default()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn hitting_sets_refuse_more_decisions_than_the_cap() {
    let many: Vec<String> = (0..=MAX_DECISIONS).map(|n| format!("d{n:02}")).collect();
    let flagged: Vec<Env> = many.iter().map(|n| env(&[n])).collect();

    let refused = hitting_sets(&flagged).unwrap_err();

    assert_eq!(refused, Unsolved::TooMany(MAX_DECISIONS + 1));
    assert!(refused.to_string().contains("11 decisions"));
    assert!(hitting_sets(&flagged[..MAX_DECISIONS]).is_ok());
}

#[test]
fn is_resolved_when_a_member_is_superseded_by_another() {
    let supersedes =
        |newer: &Assumption, older: &Assumption| newer.uid == "new" && older.uid == "old";

    assert!(is_resolved(&env(&["new", "old", "other"]), supersedes));
    // The caller gives the closure: A over B over C resolves {A, C}.
    let over = |newer: &Assumption, older: &Assumption| {
        let rank = |a: &Assumption| "abc".find(a.uid.as_str()).unwrap();
        rank(newer) < rank(older)
    };
    assert!(is_resolved(&env(&["a", "c"]), over));
    assert!(!is_resolved(&env(&["new", "other"]), supersedes));
    assert!(!is_resolved(&env(&["old"]), supersedes));
}
