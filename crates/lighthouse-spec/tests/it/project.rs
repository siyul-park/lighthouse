use std::{path::Path, sync::Arc, time::Duration};

use lighthouse_model::{Safety, Severity};
use lighthouse_resource::{Format, Metadata, Resource};
use lighthouse_spec::{
    Config, Error, Level, PluginRef, ProjectSpec, Projects, RuleConfig, RuleSetting, Rules,
    glob_set,
};
use serde_json::json;

fn rule(level: Option<Severity>, options: &[(&str, i64)]) -> RuleConfig {
    RuleConfig {
        options: options
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).into()))
            .collect(),
        ..RuleConfig::new(level)
    }
}

/// The way a project's document writes `config`.
fn setting(config: &RuleConfig) -> RuleSetting {
    let level = config.level.map_or("off".to_owned(), |l| l.to_string());
    serde_json::from_value(json!({
        "level": level,
        "options": config.options,
        "generated": config.generated,
    }))
    .unwrap()
}

/// The options `languages.<id>` hands to its provider.
fn options<'a>(config: &'a Config, id: &str) -> &'a serde_json::Map<String, serde_json::Value> {
    config
        .languages()
        .find(|(language, _)| *language == id)
        .map(|(_, options)| options)
        .unwrap()
}

fn shared(name: &str, extends: &[&str], rules: Rules) -> Resource<ProjectSpec> {
    Resource::new(
        Metadata::named(name),
        ProjectSpec {
            extends: extends.iter().map(|e| (*e).to_owned()).collect(),
            rules: rules
                .iter()
                .map(|(id, config)| (id.clone(), setting(config)))
                .collect(),
            ..ProjectSpec::default()
        },
    )
}

fn projects() -> Projects {
    Projects::new([shared(
        "core/recommended",
        &[],
        Rules::from([
            (
                "core/a".to_owned(),
                rule(Some(Severity::Warn), &[("max", 5)]),
            ),
            ("core/b".to_owned(), rule(Some(Severity::Info), &[])),
        ]),
    )])
    .unwrap()
}

fn resolve(text: &str, path: &str, lang: &str) -> Arc<Rules> {
    Config::parse_inline(text)
        .unwrap()
        .resolve(Path::new(path), lang, &projects())
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
rules = { "core/a" = "error" }
"#;

#[test]
fn config_resolve() {
    let got = resolve(
        r#"
extends = ["core/recommended"]
[rules]
"core/a" = { level = "error", options = { max = 9 } }
"core/c" = "info"
"#,
        "x.rs",
        "rust",
    );
    assert_eq!(got["core/a"], rule(Some(Severity::Error), &[("max", 9)]));
    assert_eq!(got["core/b"], rule(Some(Severity::Info), &[]));
    assert_eq!(got["core/c"], rule(Some(Severity::Info), &[]));

    let got = resolve(
        "extends = [\"core/recommended\"]\n[rules]\n\"core/a\" = \"error\"\n",
        "x.rs",
        "rust",
    );
    assert_eq!(got["core/a"], rule(Some(Severity::Error), &[("max", 5)]));

    let single = Projects::new([shared(
        "p/x",
        &[],
        Rules::from([(
            "core/a".to_owned(),
            rule(Some(Severity::Warn), &[("max", 5), ("depth", 1)]),
        )]),
    )])
    .unwrap();
    let config = Config::parse_inline(
        r#"
extends = ["p/x"]
[rules]
"core/a" = { level = "warn", options = { max = 9, extra = 2 } }
[[overrides]]
files = ["src/**"]
rules = { "core/a" = { level = "error", options = { depth = 7 } } }
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
    assert_eq!(level("internal/x.go", "go"), Some(Severity::Error));

    let config = Config::parse_inline("extends = [\"nope/x\"]").unwrap();
    let err = config
        .resolve(Path::new("x"), "text", &projects())
        .unwrap_err();
    assert!(matches!(err, Error::UnknownProject(id) if id == "nope/x"));
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
    let msg = |text: &str| Config::parse_inline(text).unwrap_err().to_string();
    assert!(msg("[rules]\n\"a/b\" = \"loud\"").contains("unknown level `loud`"));
    assert!(msg("[rules]\n\"a/b\" = \"review\"").contains("use `info`"));
    assert!(msg("[rules]\n\"a/b\" = { options = { max = 1 } }").contains("missing field `level`"));
    assert!(msg("[rules]\n\"a/b\" = 3").contains("level string or a table"));
    assert!(msg("bogus = 1").contains("unknown field `bogus`"));
    assert!(msg("[[overrides]]\nfiles = [\"[\"]").contains("invalid glob `[`"));
    assert!(msg("plugins = [3]").contains("a plugin is an id or a table with an `id`"));
    assert!(msg("plugins = [{ path = \"x\" }]").contains("missing field `id`"));
    assert!(msg("plugins = [{ id = \"x\", other = 1 }]").contains("unknown field `other`"));
    assert!(msg("plugins = [{ id = \"x\", timeout = \"soon\" }]").contains("not a duration"));
}

#[test]
fn config_configured() {
    let config = Config::parse_inline(
        "[rules]\n\"a/x\" = \"warn\"\n[[overrides]]\nrules = { \"b/y\" = \"off\" }\n",
    )
    .unwrap();
    let ids: Vec<_> = config.configured().map(|(id, _)| id).collect();
    assert_eq!(ids, ["a/x", "b/y"]);
}

const ENVELOPE: &str = r#"
apiVersion = "lighthouse/v1alpha1"
kind = "Project"
[metadata]
name = "demo"
[spec]
plugins = ["core"]
"#;

#[test]
fn config_discover_walks_up_to_the_nearest_file() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(dir.path().join("lighthouse.toml"), ENVELOPE).unwrap();
    let (path, config) = Config::discover(&nested).unwrap().unwrap();
    assert_eq!(path, dir.path().join("lighthouse.toml"));
    assert_eq!(config.plugins(), [PluginRef::new("core")]);
    assert_eq!(config.name(), "demo");
}

#[test]
fn config_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lighthouse.toml");
    assert!(matches!(Config::load(&path), Err(Error::Io { .. })));
    std::fs::write(&path, "[spec]\nextends = [\"core/recommended\"]").unwrap();
    assert!(matches!(Config::load(&path), Err(Error::Document(_))));
    std::fs::write(
        &path,
        ENVELOPE.replace("plugins = [\"core\"]", "extends = [\"core/recommended\"]"),
    )
    .unwrap();
    assert_eq!(Config::load(&path).unwrap().extends(), ["core/recommended"]);
}

#[test]
fn the_same_project_reads_from_yaml_toml_and_json() {
    let yaml = "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata: {name: demo}\nspec:\n  plugins: [core, {id: lang-go, timeout: 30s}]\n  rules:\n    core/a: {level: warn, options: {max: 3}}\n";
    let json = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Project","metadata":{"name":"demo"},"spec":{"plugins":["core",{"id":"lang-go","timeout":"30s"}],"rules":{"core/a":{"level":"warn","options":{"max":3}}}}}"#;
    let toml = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Project\"\n[metadata]\nname = \"demo\"\n[spec]\nplugins = [\"core\", { id = \"lang-go\", timeout = \"30s\" }]\n[spec.rules]\n\"core/a\" = { level = \"warn\", options = { max = 3 } }\n";
    let seen: Vec<_> = [
        (Format::Yaml, yaml),
        (Format::Toml, toml),
        (Format::Json, json),
    ]
    .into_iter()
    .map(|(format, text)| {
        let config = Config::parse_as(format, "demo", text).unwrap();
        let rules = config.resolve(Path::new("x"), "text", &projects()).unwrap();
        (config.plugins().to_vec(), rules)
    })
    .collect();
    assert_eq!(seen[0], seen[1]);
    assert_eq!(seen[1], seen[2]);
    assert_eq!(seen[0].0[1].timeout(), Some(Duration::from_secs(30)));
    assert_eq!(
        seen[0].1["core/a"],
        rule(Some(Severity::Warn), &[("max", 3)])
    );
}

#[test]
fn config_plugins() {
    let config = Config::parse_inline(
        "plugins = [\"core\", { id = \"lang-go\", path = \"tools/go\", timeout = \"30s\" }]",
    )
    .unwrap();
    let plugins = config.plugins();
    assert_eq!(plugins[0], PluginRef::new("core"));
    assert_eq!(plugins[1].id, "lang-go");
    assert_eq!(plugins[1].path.as_deref(), Some(Path::new("tools/go")));
}

#[test]
fn plugin_ref_timeout_is_a_duration_string() {
    let config =
        Config::parse_inline("plugins = [\"a\", { id = \"b\", timeout = \"2m\" }]").unwrap();
    assert_eq!(config.plugins()[0].timeout(), None);
    assert_eq!(
        config.plugins()[1].timeout(),
        Some(Duration::from_secs(120))
    );
    let error = Config::parse_inline("plugins = [{ id = \"b\", timeout = 30 }]").unwrap_err();
    assert!(error.to_string().contains("timeout"), "{error}");
    let error = Config::parse_inline("plugins = [{ id = \"b\", timeout = \"30\" }]").unwrap_err();
    assert!(error.to_string().contains("timeout"), "{error}");
}

#[test]
fn config_lists() {
    let config = Config::parse_inline("plugins = [\"lang-go\"]").unwrap();
    assert!(config.lists("lang-go") && !config.lists("design"));
}

#[test]
fn config_languages() {
    let config = Config::parse_inline("[languages.go]\ntags = [\"integration\"]\n").unwrap();
    assert_eq!(options(&config, "go")["tags"][0], "integration");
}

#[test]
fn source_rules_use_one_language_field_for_provider_and_semantics() {
    let config = Config::parse_inline(
        r#"
[[sources]]
id = "condition"
files = ["decisions/**/*.yaml"]
language = { provider = "cel" }
select = "$.spec.check.where"
"#,
    )
    .unwrap();
    assert_eq!(config.sources().len(), 1);
    assert_eq!(
        config.sources()[0].language.provider.as_deref(),
        Some("cel")
    );
    assert!(
        Config::parse_inline(
            r#"
[[sources]]
id = "condition"
files = ["decisions/**/*.yaml"]
host = "yaml"
language = { provider = "cel" }
select = "$.spec.check.where"
"#
        )
        .is_err()
    );
    assert!(
        Config::parse_inline(
            r#"[[sources]]
id = "condition"
files = ["decisions/**/*.yaml"]
select = "$.spec.check.where"
language = { provider = "cel", from = "$.language" }
"#
        )
        .is_err()
    );
    for language in [
        r#"{ provider = "", from = "$.language" }"#,
        r#"{ provider = "cel", from = "" }"#,
        r#"{ from = "" }"#,
        r#"{}"#,
    ] {
        let source = format!(
            r#"[[sources]]
id = "condition"
files = ["decisions/**/*.yaml"]
select = "$.spec.check.where"
language = {language}
"#
        );
        assert!(
            Config::parse_inline(&source).is_err(),
            "an empty or missing provider/from must be rejected: {language}"
        );
    }
    let named = Config::parse_inline(
        r#"[[sources]]
id = "fences"
files = ["**/*.md"]
select = "$..?(@.kind == 'code_block')"
language = { from = "$.lang", context = { example = true } }
"#,
    )
    .unwrap();
    assert_eq!(named.sources()[0].language.from.as_deref(), Some("$.lang"));
    assert_eq!(named.sources()[0].language.provider, None);
    assert!(
        Config::parse_inline(
            r#"[[sources]]
id = "condition"
files = ["decisions/**/*.yaml"]
select = "$?(@.kind = 'Decision').spec.check.where"
language = { provider = "cel" }
"#
        )
        .is_err(),
        "a malformed selector must fail config loading"
    );
    for context in [
        r#"{ variable = { name = "$.name" } }"#,
        r#"{ identity = {} }"#,
        r#"{ variable = { value = "$.expression", extra = true } }"#,
        r#"{ variable = { value = "$.expression", shared = "yes" } }"#,
        r#"{ declared = { select = "$.kind", ignored = true } }"#,
    ] {
        let source = format!(
            r#"
[[sources]]
id = "condition"
files = ["decisions/**/*.yaml"]
select = "$"
language = {{ provider = "cel", context = {context} }}
"#
        );
        assert!(
            Config::parse_inline(&source).is_err(),
            "invalid source context should fail config loading: {context}"
        );
    }
}

#[test]
fn config_extends() {
    let config = Config::parse_inline("extends = [\"a/x\", \"b/y\"]").unwrap();
    assert_eq!(config.extends(), ["a/x", "b/y"]);
}

#[test]
fn a_fix_table_in_the_repository_is_refused_because_trust_is_the_users() {
    assert!(Config::parse_inline("[fix]\ncommands = \"allow\"\n").is_err());
}

#[test]
fn a_rule_gives_its_fixes_a_safety_that_the_unsafe_of_a_later_layer_beats() {
    let safety = |text: &str| {
        let config = Config::parse_inline(text).unwrap();
        let rules = config
            .resolve(Path::new("a.go"), "go", &Projects::default())
            .unwrap();
        rules.get("d/x").and_then(|rule| rule.safety)
    };
    let rule = |safety: &str| format!("[rules.\"d/x\"]\nlevel = \"warn\"\nsafety = \"{safety}\"\n");

    assert_eq!(safety(&rule("safe")), Some(Safety::Safe));
    assert_eq!(safety(&rule("unsafe")), Some(Safety::Unsafe));
    assert_eq!(safety("[rules.\"d/x\"]\nlevel = \"warn\"\n"), None);
    let later = |first: &str, second: &str| {
        safety(&format!(
            "{}[[overrides]]\nfiles = [\"**/*.go\"]\n[overrides.rules.\"d/x\"]\nlevel = \"warn\"\nsafety = \"{second}\"\n",
            rule(first)
        ))
    };
    assert_eq!(later("unsafe", "safe"), Some(Safety::Unsafe), "unsafe wins");
    assert_eq!(later("safe", "unsafe"), Some(Safety::Unsafe));
    assert_eq!(later("safe", "safe"), Some(Safety::Safe));
    assert!(
        Config::parse_inline("[rules.\"d/x\"]\nlevel = \"warn\"\nsafety = \"maybe\"\n").is_err()
    );
}

#[test]
fn the_kinds_of_a_configuration_are_described_by_schemas() {
    let kinds: Vec<&str> = lighthouse_spec::descriptors()
        .iter()
        .map(|d| d.kind)
        .collect();

    assert!(kinds.contains(&"Project"));
    assert!(!kinds.contains(&"Preset"));
}

#[test]
fn a_parsed_project_builds_a_config() {
    let document = Resource::new(
        Metadata::named("built"),
        lighthouse_spec::ProjectSpec {
            extends: vec!["core/recommended".to_owned()],
            rules: [(
                "core/a".to_owned(),
                lighthouse_spec::RuleSetting::Level(lighthouse_spec::Level::Off),
            )]
            .into(),
            ..lighthouse_spec::ProjectSpec::default()
        },
    );

    let config = Config::from_resource(document).unwrap();

    assert_eq!(config.name(), "built");
    assert_eq!(config.extends(), ["core/recommended"]);
    let rules = config.resolve(Path::new("x"), "text", &projects()).unwrap();
    assert_eq!(rules["core/a"].level, None);
}

#[test]
fn rule_settings_and_levels_convert_both_ways() {
    let plain: RuleSetting = serde_json::from_value(json!("warn")).unwrap();
    let detailed: RuleSetting =
        serde_json::from_value(json!({ "level": "error", "options": { "max": 3 } })).unwrap();

    assert_eq!(plain, RuleSetting::Level(Level::Warn));
    assert!(
        matches!(detailed, RuleSetting::Detailed(ref d) if d.level == Level::Error && d.options["max"] == 3)
    );
    assert_eq!(
        RuleConfig::from(detailed),
        rule(Some(Severity::Error), &[("max", 3)])
    );
    assert_eq!(Option::<Severity>::from(Level::Off), None);
    assert_eq!(Level::from(Some(Severity::Info)), Level::Info);
}

#[test]
fn a_configuration_file_is_found_in_a_directory_by_any_of_its_names() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Config::file_in(dir.path()), None);

    std::fs::write(dir.path().join("lighthouse.json"), "{}").unwrap();
    assert_eq!(
        Config::file_in(dir.path()),
        Some(dir.path().join("lighthouse.json"))
    );
    std::fs::write(dir.path().join("lighthouse.toml"), "").unwrap();
    assert_eq!(
        Config::file_in(dir.path()),
        Some(dir.path().join("lighthouse.toml"))
    );
}

#[test]
fn a_project_starts_from_the_projects_it_extends() {
    let level = |severity| Rules::from([("p/a".to_owned(), rule(Some(severity), &[]))]);
    let projects = Projects::new([
        shared("p/base", &[], level(Severity::Warn)),
        shared("p/strict", &["p/base"], level(Severity::Error)),
    ])
    .unwrap();
    let config = Config::parse_inline("extends = [\"p/strict\"]").unwrap();

    let rules = config.resolve(Path::new("x"), "text", &projects).unwrap();

    assert_eq!(rules["p/a"].level, Some(Severity::Error));
}

#[test]
fn an_extended_project_keeps_its_overrides() {
    let mut base = shared("p/base", &[], Rules::new());
    base.spec.overrides = vec![lighthouse_spec::OverrideSpec {
        files: vec!["gen/**".to_owned()],
        rules: [("p/a".to_owned(), RuleSetting::Level(Level::Info))].into(),
        ..lighthouse_spec::OverrideSpec::default()
    }];
    let projects = Projects::new([base]).unwrap();
    let config = Config::parse_inline("extends = [\"p/base\"]\n[rules]\n\"p/a\" = \"error\"\n[[overrides]]\nfiles = [\"vendor/**\"]\nrules = { \"p/a\" = \"off\" }").unwrap();

    let at = |path| config.resolve(Path::new(path), "text", &projects).unwrap()["p/a"].level;

    assert_eq!(at("src/x.rs"), Some(Severity::Error));
    assert_eq!(at("vendor/x.rs"), None);
    // The project's own rules come after what it extends, overrides included.
    assert_eq!(at("gen/x.rs"), Some(Severity::Error));
}

#[test]
fn projects_that_extend_a_stranger_or_each_other_have_no_rules() {
    let projects = Projects::new([
        shared("p/lost", &["p/nope"], Rules::new()),
        shared("p/one", &["p/two"], Rules::new()),
        shared("p/two", &["p/one"], Rules::new()),
    ])
    .unwrap();
    let resolve = |name: &str| {
        Config::parse_inline(&format!("extends = [\"{name}\"]"))
            .unwrap()
            .resolve(Path::new("x"), "text", &projects)
            .unwrap_err()
    };

    assert!(matches!(resolve("p/lost"), Error::UnknownProject(id) if id == "p/nope"));
    assert!(matches!(resolve("p/one"), Error::ProjectCycle(_)));
    assert!(matches!(resolve("p/missing"), Error::UnknownProject(_)));
}

#[test]
fn two_projects_of_one_name_are_refused() {
    let twice = Projects::new([
        shared("p/a", &[], Rules::new()),
        shared("p/a", &[], Rules::new()),
    ]);

    assert!(matches!(twice, Err(Error::DuplicateProject(name)) if name == "p/a"));
}

#[test]
fn the_entries_of_a_project_are_those_of_what_it_extends_and_its_own() {
    let config =
        Config::parse_inline("extends = [\"core/recommended\"]\n[rules]\n\"core/c\" = \"info\"")
            .unwrap();

    let known = projects();
    let entries = config.entries(&known).unwrap();

    let ids: Vec<&str> = entries.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, ["core/a", "core/b", "core/c"]);
}

const GENERATED: &str = "[generated]\nfiles = [\"**/*.pb.go\"]\ncheck = \"skip\"\n";

#[test]
fn config_is_generated() {
    let config = Config::parse_inline(GENERATED).unwrap();

    assert!(config.is_generated(Path::new("api/x.pb.go")));
    assert!(!config.is_generated(Path::new("api/x.go")));
    assert!(
        !Config::parse_inline("")
            .unwrap()
            .is_generated(Path::new("x.pb.go"))
    );
}

#[test]
fn config_generated_check() {
    use lighthouse_spec::GeneratedCheck;

    let config = Config::parse_inline(GENERATED).unwrap();
    assert_eq!(config.generated_check(), Some(GeneratedCheck::Skip));
    let include = Config::parse_inline("[generated]\ncheck = \"include\"").unwrap();
    assert_eq!(include.generated_check(), Some(GeneratedCheck::Include));
    assert_eq!(Config::parse_inline("").unwrap().generated_check(), None);
}

#[test]
fn a_rule_can_say_whether_it_checks_generated_code() {
    let config = Config::parse_inline(
        "[rules]\n\"core/a\" = { level = \"warn\", generated = true }\n\"core/b\" = \"warn\"",
    )
    .unwrap();

    let rules = config.resolve(Path::new("x"), "text", &projects()).unwrap();

    assert_eq!(rules["core/a"].generated, Some(true));
    assert_eq!(rules["core/b"].generated, None);
}

#[test]
fn config_constructor_prefixes() {
    let config = Config::parse_inline(
        "[languages.go]\nconstructorPrefixes = [\"Make\"]\n[languages.rust]\ntags = 1\n",
    )
    .unwrap();
    let prefixes: Vec<_> = config.constructor_prefixes().collect();
    assert_eq!(prefixes, [("go", &["Make".to_owned()][..])]);
    assert!(!options(&config, "go").contains_key("constructorPrefixes"));
}

#[test]
fn config_test_naming() {
    let config = Config::parse_inline(
        "[languages.rust]\ntestNaming = { prefix = \"t_\", snakeCase = true }\n[languages.go]\ntags = 1\n",
    )
    .unwrap();

    let naming: Vec<_> = config.test_naming().collect();

    assert_eq!(naming.len(), 1);
    assert_eq!(naming[0].0, "rust");
    assert_eq!(naming[0].1.prefix.as_deref(), Some("t_"));
    assert_eq!(naming[0].1.snake_case, Some(true));
    assert_eq!(naming[0].1.variants, None);
    assert!(!options(&config, "rust").contains_key("testNaming"));
}

#[test]
fn object_options_merge_per_key_across_layers_in_toml() {
    let limits = RuleConfig {
        options: serde_json::from_value(json!({ "max": { "default": 5, "test": -1 } })).unwrap(),
        ..RuleConfig::new(Some(Severity::Warn))
    };
    let layers = Projects::new([shared(
        "base/limits",
        &[],
        Rules::from([("core/a".to_owned(), limits)]),
    )])
    .unwrap();
    let text = "extends = [\"base/limits\"]\n[rules]\n\"core/a\" = { level = \"warn\", options = { max = { default = 8 } } }\n";
    let rules = Config::parse_inline(text)
        .unwrap()
        .resolve(Path::new("a.rs"), "rust", &layers)
        .unwrap();
    let max = &rules["core/a"].options["max"];
    assert_eq!(max["default"], 8, "the later layer wins per key");
    assert_eq!(max["test"], -1, "and keeps the keys it does not name");
}
