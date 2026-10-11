//! A `command` model provider run as a classifier.

use std::collections::BTreeMap;

use lighthouse_plugin::{Classifier, DecisionRef, Question, State, SystemOneRequest};
use lighthouse_rpc::Command;
use lighthouse_spec::Config;

fn provider(argv: &str) -> lighthouse_spec::ModelProvider {
    let config = Config::parse_inline(&format!(
        "plugins = []\n[models.providers.p]\ntype = \"command\"\nargv = {argv}\n"
    ))
    .unwrap();
    config.models().provider("p").unwrap()
}

fn request() -> SystemOneRequest {
    let decision = DecisionRef {
        id: "d/x".to_owned(),
        requirement: "r".to_owned(),
        prompt: "r".to_owned(),
        valid: Vec::new(),
        invalid: Vec::new(),
    };
    SystemOneRequest {
        state: State {
            language: "go".to_owned(),
            file: "a.go".to_owned(),
            text: String::new(),
            facts: serde_json::Value::Null,
            neighbors: Vec::new(),
        },
        model: String::new(),
        questions: BTreeMap::from([("d/x".to_owned(), Question::of(&decision))]),
        options: serde_json::Map::new(),
    }
}

#[test]
fn command_new_runs_a_program_that_prints_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "cat >/dev/null; printf '{\"noul\": 0.75}'"]"#);

    let command = Command::new(dir.path(), "p/m", "m", &provider).unwrap();
    let answer = command.classify(&request()).unwrap();

    assert_eq!(command.manifest().id, "p/m");
    assert_eq!(answer.answers["d/x"].noul, 0.75);
}

#[test]
fn command_new_fails_a_program_that_exits_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "exit 3"]"#);

    let command = Command::new(dir.path(), "p/m", "m", &provider).unwrap();

    assert!(command.classify(&request()).is_err());
}

#[test]
fn command_new_is_none_for_a_provider_that_is_not_a_command() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_inline(
        "plugins = []\n[models.providers.p]\ntype = \"rpc\"\nplugin = \"x\"\n",
    )
    .unwrap();
    let provider = config.models().provider("p").unwrap();

    assert!(Command::new(dir.path(), "p/m", "m", &provider).is_none());
}
