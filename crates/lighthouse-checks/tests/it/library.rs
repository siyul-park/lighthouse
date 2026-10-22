//! The functions the model adds to CEL, each against the example it is
//! documented with, and the `words` field of a node.

use crate::{
    rules::local,
    support::{Subject, World},
};
use lighthouse_model::ucm::SymbolKind;
use serde_json::json;

/// What `expression` is, as the message of an always-true check of the symbol
/// `name`.
fn evaluated_for(name: &str, expression: &str) -> String {
    let plugin = local(
        "symbol",
        &format!("select: symbol\nwhere: 'true'\nmessage: \"{{{{ {expression} }}}}\""),
    )
    .unwrap();
    let mut w = World::default();
    w.symbol("m", name, SymbolKind::Var, "m/a.ucm");
    w.messages_in(
        &Subject {
            plugin: &plugin,
            id: "local",
        },
        "local/probe",
        json!({}),
    )
    .remove(0)
}

fn evaluated(expression: &str) -> String {
    evaluated_for("A", expression)
}

#[test]
fn matches_glob_matches_module_paths_by_segment() {
    assert_eq!(evaluated("'app/http'.matchesGlob('app/*')"), "true");
    assert_eq!(evaluated("'app/http/x'.matchesGlob('app/*')"), "false");
    assert_eq!(evaluated("'app'.matchesGlob('app/**')"), "true");
}

#[test]
fn layer_index_is_the_index_of_the_first_layer_with_a_matching_glob() {
    let layers = "[['app/**'], ['domain/**', 'core']]";
    assert_eq!(
        evaluated(&format!("'domain/order'.layerIndex({layers})")),
        "1"
    );
    assert_eq!(evaluated(&format!("'infra'.layerIndex({layers})")), "-1");
}

#[test]
fn misplaced_leaves_the_first_longest_run_in_place() {
    let moved = |list: &str| evaluated(&format!("{list}.misplaced().map(i, string(i)).join(',')"));
    assert_eq!(moved("[3, 0, 1, 2]"), "0");
    assert_eq!(moved("[10, 10, 0]"), "2");
    assert_eq!(moved("[1, 0]"), "1");
    assert_eq!(moved("[]"), "");
    assert_eq!(moved("[0, 1, 1, 2]"), "");
    assert_eq!(moved("[-1, -5, -3, -3]"), "0");
    assert_eq!(moved("[0, -1, -1, 5, -2]"), "0,4");
}

#[test]
fn misplaced_is_empty_exactly_when_the_list_is_in_order() {
    for list in ["[]", "[2]", "[1, 1, 3]", "[3, 2]", "[1, 3, 2]", "[2, 1, 1]"] {
        assert_eq!(
            evaluated(&format!("{list}.misplaced().size() == 0")),
            evaluated(&format!("{list} == {list}.sortBy(k, k)")),
            "{list}"
        );
    }
}

#[test]
fn misplaced_and_anchors_refuse_a_list_that_holds_other_than_ints() {
    for expression in ["['a'].misplaced().size()", "[1, 'a'].anchors().size()"] {
        let failed = std::panic::catch_unwind(|| evaluated(expression)).unwrap_err();
        let message = failed
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| failed.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        assert!(message.contains("holds ints"), "{expression}: {message}");
    }
}

#[test]
fn anchors_is_the_index_of_the_first_later_element_that_sorts_before() {
    let found = |list: &str| evaluated(&format!("{list}.anchors().map(i, string(i)).join(',')"));
    assert_eq!(found("[3, 1, 2, 0]"), "1,3,3,-1");
    assert_eq!(found("[1, 1, 1]"), "-1,-1,-1");
    assert_eq!(found("[]"), "");
}

/// The longest run by the definition, to compare the fast one with: the
/// first longest run, each element after its earliest best predecessor; what
/// it leaves out is what `misplaced` says.
#[test]
fn misplaced_agrees_with_the_quadratic_definition_on_many_lists() {
    let mut seed = 7_u64;
    let mut next = move |bound: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % bound
    };
    for _ in 0..40 {
        let n = 1 + usize::try_from(next(14)).unwrap();
        let ranks: Vec<i64> = (0..n)
            .map(|_| i64::try_from(next(6)).unwrap() - 2)
            .collect();
        let (mut best, mut from) = (vec![1usize; n], vec![usize::MAX; n]);
        for i in 0..n {
            for j in 0..i {
                if ranks[j] <= ranks[i] && best[j] + 1 > best[i] {
                    best[i] = best[j] + 1;
                    from[i] = j;
                }
            }
        }
        let mut kept = Vec::new();
        let mut at = (0..n).max_by_key(|&i| (best[i], std::cmp::Reverse(i)));
        while let Some(i) = at {
            kept.push(i);
            at = Some(from[i]).filter(|&j| j != usize::MAX);
        }
        let expected: Vec<String> = (0..n)
            .filter(|i| !kept.contains(i))
            .map(|i| i.to_string())
            .collect();
        let list = format!("{ranks:?}");
        assert_eq!(
            evaluated(&format!("{list}.misplaced().map(i, string(i)).join(',')")),
            expected.join(","),
            "{list}"
        );
    }
}

#[test]
fn the_words_of_a_symbol_are_the_words_of_its_name() {
    assert_eq!(
        evaluated_for("parseHTTPServer", "symbol.words.join(' ')"),
        "parse http server"
    );
    assert_eq!(
        evaluated_for("max_open_files", "symbol.words.join(' ')"),
        "max open files"
    );
}
