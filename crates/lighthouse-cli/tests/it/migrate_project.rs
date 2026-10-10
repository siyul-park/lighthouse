use std::{path::Path, time::Duration};

use lighthouse_config::{Config, Format, FormatterOutput, RuleConfig, Rules};
use lighthouse_model::Severity;
use lighthouse_session::migrate::project::{is_legacy, migrate};

fn rule(level: Option<Severity>, options: &[(&str, i64)]) -> RuleConfig {
    RuleConfig {
        level,
        options: options
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).into()))
            .collect(),
    }
}

fn presets(id: &str) -> Option<Rules> {
    (id == "core/recommended").then(|| {
        Rules::from([
            (
                "core/a".to_owned(),
                rule(Some(Severity::Warn), &[("max", 5)]),
            ),
            ("core/b".to_owned(), rule(Some(Severity::Info), &[])),
        ])
    })
}

#[test]
fn an_old_configuration_migrates_to_a_project() {
    let old: serde_json::Value = toml::from_str(
        r#"
plugins = ["core", { id = "lang-go", timeout = 30 }]
extends = ["core/recommended"]
[languages.go]
formatter = { argv = ["gofmt"], output = "inPlace" }
tags = ["x"]
[rules]
"core/a" = { level = "review", max = 9 }
"core/b" = "review"
[[overrides]]
files = ["x/**"]
rules = { "core/b" = "off", "core/c" = { level = "warn", depth = 2 } }
"#,
    )
    .unwrap();

    let migrated = migrate(&old, "demo").unwrap();

    assert_eq!(migrated["kind"], "Project");
    assert_eq!(migrated["metadata"]["name"], "demo");
    let config = Config::parse_as(Format::Json, "demo", &migrated.to_string()).unwrap();
    assert_eq!(config.plugins()[1].timeout(), Some(Duration::from_secs(30)));
    assert_eq!(
        config.formatter("go").unwrap().output,
        FormatterOutput::InPlace
    );
    assert_eq!(config.languages()["go"]["tags"][0], "x");
    let rules = config.resolve(Path::new("a.go"), "go", &presets).unwrap();
    assert_eq!(rules["core/a"], rule(Some(Severity::Info), &[("max", 9)]));
    assert_eq!(rules["core/b"], rule(Some(Severity::Info), &[]));
    let in_x = config.resolve(Path::new("x/a.go"), "go", &presets).unwrap();
    assert_eq!(in_x["core/b"].level, None);
    assert_eq!(in_x["core/c"], rule(Some(Severity::Warn), &[("depth", 2)]));
    // Only a table can be migrated.
    assert!(migrate(&serde_json::json!(3), "x").is_err());
}

#[test]
fn is_legacy_recognizes_the_keys_of_the_old_format() {
    assert!(is_legacy(&serde_json::json!({ "plugins": ["core"] })));
    assert!(!is_legacy(&serde_json::json!({ "name": "not ours" })));
    assert!(!is_legacy(&serde_json::json!(["plugins"])));
}
