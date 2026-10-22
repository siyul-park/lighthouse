//! The `events` of an agent provider: a CEL mapping of its stdout, checked
//! when the configuration is read, and the recipes that are configuration
//! layers declaring one.

use std::fs;

use lighthouse_spec::{Config, Layer, ModelProvider, Projects};

const MAPPING: &str = "[models.providers.p.events]
select = 'line.type == \"call\"'
tool = 'tools[line.name]'
path = 'line.?file.orValue(\"\")'
tools = { open = \"read\", edit = \"edit\" }
";

fn invalid(text: &str) -> String {
    let models = Config::parse_inline(&format!("[models.providers.p]\n{text}"))
        .unwrap()
        .models(&Projects::default())
        .unwrap();
    models.invalid["p"].clone()
}

#[test]
fn an_agent_declares_how_its_stdout_is_read() {
    let config = Config::parse_inline(&format!(
        "[models.providers.p]\ncommand = [\"a\"]\n{MAPPING}"
    ))
    .unwrap();

    let provider = config
        .models(&Projects::default())
        .unwrap()
        .provider("p")
        .unwrap();

    let ModelProvider::Command { events, .. } = provider else {
        panic!("a command provider");
    };
    let events = events.unwrap();
    assert_eq!(events.select, "line.type == \"call\"");
    assert_eq!(events.tools["open"], "read");
}

#[test]
fn a_mapping_that_cannot_work_is_refused_naming_the_key() {
    let broken = "command = [\"a\"]\n[models.providers.p.events]\nselect = 'line.type =='\ntool = 'tools[line.name]'\n";
    assert!(
        invalid(broken).contains("`events.select`"),
        "{}",
        invalid(broken)
    );

    let unknown = MAPPING.replace("edit = \"edit\"", "edit = \"change\"");
    let text = format!("command = [\"a\"]\n{unknown}");
    assert!(
        invalid(&text).contains("`events.tools.edit` is `change`, not a tool"),
        "{}",
        invalid(&text)
    );
}

#[test]
fn only_an_agent_has_events() {
    let classifier = format!("protocol = \"typesafe\"\ncommand = [\"a\"]\n{MAPPING}");
    assert!(
        invalid(&classifier).contains("only a `command` without a `protocol`"),
        "{}",
        invalid(&classifier)
    );

    let http = format!("protocol = \"openai\"\nurl = \"http://x\"\n{MAPPING}");
    assert!(
        invalid(&http).contains("has no `events`"),
        "{}",
        invalid(&http)
    );
}

#[test]
fn every_recipe_is_a_configuration_layer_that_declares_one_agent_with_events() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../recipes");
    let mut found = 0;
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let file = entry.path().join("recipe.toml");
        let layer = Layer::parse(
            &file.display().to_string(),
            &fs::read_to_string(&file).unwrap(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let providers = &layer.spec.models.providers;
        assert_eq!(providers.keys().collect::<Vec<_>>(), [&name]);
        let ModelProvider::Command { events, .. } = providers[&name].resolve().unwrap() else {
            panic!("{name} is not a command provider");
        };
        assert!(events.is_some(), "{name} declares no events");
        found += 1;
    }
    assert_eq!(found, 4);
}

#[test]
fn a_mapping_with_an_expression_that_does_not_compile_has_a_problem_naming_its_key() {
    let spec = lighthouse_spec::EventsSpec {
        select: "line.name ==".to_owned(),
        tool: "tools[line.name]".to_owned(),
        ..Default::default()
    };

    assert!(spec.problem().unwrap().contains("`events.select`"));
}

#[test]
fn expressions_lists_the_keys_that_are_set_with_their_source() {
    let spec = lighthouse_spec::EventsSpec {
        select: "true".to_owned(),
        tool: "\"read\"".to_owned(),
        path: Some("line.p".to_owned()),
        ..lighthouse_spec::EventsSpec::default()
    };

    let keys: Vec<_> = spec.expressions().map(|(key, _)| key).collect();

    assert_eq!(keys, ["select", "tool", "path"]);
}
