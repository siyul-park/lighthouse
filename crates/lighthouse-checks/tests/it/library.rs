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
