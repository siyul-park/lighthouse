use std::path::Path;
use std::time::Duration;

use lighthouse_config::{Config, Error, PluginRef, RuleConfig, Rules, glob_set};
use lighthouse_model::Severity;

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

fn resolve(text: &str, path: &str, lang: &str) -> Rules {
    Config::parse(text)
        .unwrap()
        .resolve(Path::new(path), lang, &presets)
        .unwrap()
}

const LAYERED: &str = r#"
[rules]
"core/a" = "warn"
[[overrides]]
files = ["internal/**"]
rules = { "core/a" = "error" }
[[overrides]]
languages = ["go"]
rules = { "core/a" = "info" }
[[overrides]]
files = ["internal/**"]
languages = ["go"]
rules = { "core/a" = "review" }
"#;

#[test]
fn config_resolve() {
    let got = resolve(
        r#"
extends = ["core/recommended"]
[rules]
"core/a" = { level = "error", max = 9 }
"core/c" = "review"
"#,
        "x.rs",
        "rust",
    );
    assert_eq!(got["core/a"], rule(Some(Severity::Error), &[("max", 9)]));
    assert_eq!(got["core/b"], rule(Some(Severity::Info), &[]));
    assert_eq!(got["core/c"], rule(Some(Severity::Review), &[]));

    let got = resolve(
        "extends = [\"core/recommended\"]\n[rules]\n\"core/a\" = \"error\"\n",
        "x.rs",
        "rust",
    );
    assert_eq!(got["core/a"], rule(Some(Severity::Error), &[("max", 5)]));

    let single = |_: &str| {
        Some(Rules::from([(
            "core/a".to_owned(),
            rule(Some(Severity::Warn), &[("max", 5), ("depth", 1)]),
        )]))
    };
    let config = Config::parse(
        r#"
extends = ["p/x"]
[rules]
"core/a" = { level = "warn", max = 9, extra = 2 }
[[overrides]]
files = ["src/**"]
rules = { "core/a" = { level = "error", depth = 7 } }
"#,
    )
    .unwrap();
    let got = config
        .resolve(Path::new("src/x.rs"), "rust", &single)
        .unwrap();
    assert_eq!(
        got["core/a"],
        rule(
            Some(Severity::Error),
            &[("max", 9), ("depth", 7), ("extra", 2)]
        )
    );

    let got = resolve("[rules]\n\"core/a\" = \"off\"\n", "x.rs", "rust");
    assert_eq!(got["core/a"].level, None);

    let level = |path, lang| resolve(LAYERED, path, lang)["core/a"].level;
    assert_eq!(level("pkg/x.rs", "rust"), Some(Severity::Warn));
    assert_eq!(level("internal/x.rs", "rust"), Some(Severity::Error));
    assert_eq!(level("pkg/x.go", "go"), Some(Severity::Info));
    assert_eq!(level("internal/x.go", "go"), Some(Severity::Review));

    let config = Config::parse("extends = [\"nope/x\"]").unwrap();
    let err = config
        .resolve(Path::new("x"), "text", &presets)
        .unwrap_err();
    assert!(matches!(err, Error::UnknownPreset(id) if id == "nope/x"));
}

#[test]
fn glob_set_star_stays_within_a_directory_and_double_star_crosses_them() {
    let set = glob_set(["*.rs"]).unwrap();
    assert!(set.is_match("a.rs"));
    assert!(!set.is_match("src/a.rs"));
    let set = glob_set(["**/*.rs"]).unwrap();
    assert!(set.is_match("src/deep/a.rs"));
    assert!(glob_set(["**"]).unwrap().is_match("src/deep/a.rs"));
    assert!(!glob_set(["src/*"]).unwrap().is_match("src/a/b"));
}

#[test]
fn config_parse() {
    let msg = |text: &str| Config::parse(text).unwrap_err().to_string();
    assert!(msg("[rules]\n\"a/b\" = \"loud\"").contains("unknown severity `loud`"));
    assert!(msg("[rules]\n\"a/b\" = { max = 1 }").contains("requires a string `level`"));
    assert!(msg("[rules]\n\"a/b\" = 3").contains("level string or a table"));
    assert!(msg("bogus = 1").contains("unknown field `bogus`"));
    assert!(msg("[[overrides]]\nfiles = [\"[\"]").contains("invalid glob `[`"));
    assert!(Config::parse("plugins = [{ path = \"x\" }]").is_err());
    assert!(Config::parse("plugins = [{ id = \"x\", other = 1 }]").is_err());
}

#[test]
fn config_configured() {
    let config = Config::parse(
        "[rules]\n\"a/x\" = \"warn\"\n[[overrides]]\nrules = { \"b/y\" = \"off\" }\n",
    )
    .unwrap();
    let ids: Vec<_> = config.configured().map(|(id, _)| id).collect();
    assert_eq!(ids, ["a/x", "b/y"]);
}

#[test]
fn config_discover_walks_up_to_the_nearest_file() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(dir.path().join("lighthouse.toml"), "plugins = [\"core\"]").unwrap();
    let (path, config) = Config::discover(&nested).unwrap().unwrap();
    assert_eq!(path, dir.path().join("lighthouse.toml"));
    assert_eq!(config.plugins(), [PluginRef::new("core")]);
}

#[test]
fn config_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lighthouse.toml");
    assert!(matches!(Config::load(&path), Err(Error::Io { .. })));
    std::fs::write(&path, "extends = [\"core/recommended\"]").unwrap();
    assert_eq!(Config::load(&path).unwrap().extends(), ["core/recommended"]);
}

#[test]
fn config_plugins() {
    let config = Config::parse(
        "plugins = [\"core\", { id = \"lang-go\", path = \"tools/go\", timeout = 30 }]",
    )
    .unwrap();
    let plugins = config.plugins();
    assert_eq!(plugins[0], PluginRef::new("core"));
    assert_eq!(plugins[1].id, "lang-go");
    assert_eq!(plugins[1].path.as_deref(), Some(Path::new("tools/go")));
}

#[test]
fn plugin_ref_timeout() {
    let config = Config::parse("plugins = [\"a\", { id = \"b\", timeout = 30 }]").unwrap();
    assert_eq!(config.plugins()[0].timeout(), None);
    assert_eq!(config.plugins()[1].timeout(), Some(Duration::from_secs(30)));
}

#[test]
fn config_lists() {
    let config = Config::parse("plugins = [\"lang-go\"]").unwrap();
    assert!(config.lists("lang-go") && !config.lists("design"));
}

#[test]
fn config_languages() {
    let config = Config::parse("[languages.go]\ntags = [\"integration\"]\n").unwrap();
    assert_eq!(config.languages()["go"]["tags"][0], "integration");
}

#[test]
fn config_extends() {
    let config = Config::parse("extends = [\"a/x\", \"b/y\"]").unwrap();
    assert_eq!(config.extends(), ["a/x", "b/y"]);
}
