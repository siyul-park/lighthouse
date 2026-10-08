use lighthouse_config::{RuleConfig, Rules};
use lighthouse_model::{Options, Severity};
use lighthouse_plugin::{OrderKeyManifest, Plugin, PluginManifest, PresetManifest, Registry};

fn rules(entries: &[(&str, Severity)]) -> Rules {
    entries
        .iter()
        .map(|(id, level)| {
            (
                (*id).to_owned(),
                RuleConfig {
                    level: Some(*level),
                    options: Options::new(),
                },
            )
        })
        .collect()
}

struct Presets(PluginManifest, Vec<PresetManifest>);

impl Plugin for Presets {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn presets(&self) -> Vec<PresetManifest> {
        self.1.clone()
    }
}

fn preset(id: &str, extends: &[&str], entries: &[(&str, Severity)]) -> PresetManifest {
    PresetManifest {
        id: id.to_owned(),
        extends: extends.iter().map(|e| (*e).to_owned()).collect(),
        rules: rules(entries),
    }
}

fn registry(presets: Vec<PresetManifest>) -> Registry {
    let mut registry = Registry::default();
    let manifest = PluginManifest {
        id: "p".to_owned(),
        version: "0".to_owned(),
    };
    registry.register(&Presets(manifest, presets)).unwrap();
    registry
}

#[test]
fn a_preset_starts_from_the_presets_it_extends() {
    let registry = registry(vec![
        preset(
            "p/base",
            &[],
            &[("p/a", Severity::Warn), ("p/b", Severity::Warn)],
        ),
        preset("p/strict", &["p/base"], &[("p/b", Severity::Error)]),
    ]);

    let got = registry.preset_rules("p/strict").unwrap();

    assert_eq!(got["p/a"].level, Some(Severity::Warn));
    assert_eq!(got["p/b"].level, Some(Severity::Error));
}

#[test]
fn presets_that_extend_a_stranger_or_each_other_have_no_rules() {
    let registry = registry(vec![
        preset("p/lost", &["p/nope"], &[]),
        preset("p/one", &["p/two"], &[]),
        preset("p/two", &["p/one"], &[]),
    ]);

    assert!(registry.preset_rules("p/lost").is_none());
    assert!(registry.preset_rules("p/one").is_none());
    assert!(registry.preset_rules("p/missing").is_none());
}

#[test]
fn a_preset_and_an_order_key_describe_themselves_as_documents() {
    let original = preset("p/strict", &["p/base"], &[("p/b", Severity::Error)]);

    let document = original.to_resource();

    assert_eq!(document.metadata.name, "p/strict");
    assert_eq!(PresetManifest::from_resource(document), original);
    let key = OrderKeyManifest {
        id: "p/group".to_owned(),
        description: "by group".to_owned(),
    };
    let key = serde_json::to_value(key.to_resource()).unwrap();
    assert_eq!(key["kind"], "OrderKey");
    assert_eq!(key["spec"]["description"], "by group");
}

#[test]
fn the_order_key_kind_has_a_schema() {
    let kinds: Vec<&str> = lighthouse_plugin::descriptors()
        .iter()
        .map(|d| d.kind)
        .collect();

    assert_eq!(kinds, ["OrderKey"]);
}
