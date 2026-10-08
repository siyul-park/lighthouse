use std::sync::LazyLock;

use lighthouse_model::{
    Capability, Fragment,
    annotation::{ANNOTATION_REASON, UNUSED_ALLOW},
};
use lighthouse_plugin::{
    Analyzer, Ctx, Error, Fixer, Indexed, LanguageProvider, Plugin, PluginManifest, PresetManifest,
    ProviderManifest, Rule, RuleManifest, Source, Workspace,
};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;

pub struct Core;

impl Plugin for Core {
    fn manifest(&self) -> &PluginManifest {
        static MANIFEST: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
            id: "core".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        &MANIFEST
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Text::new())]
    }

    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        Vec::new()
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        let mut rules = lighthouse_declarative::Declarative::bundled_rules("core");
        rules.push(annotation_rule(ANNOTATION_REASON));
        rules.push(annotation_rule(UNUSED_ALLOW));
        rules
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        lighthouse_declarative::Declarative::bundled_fixers("core")
    }

    fn presets(&self) -> Vec<PresetManifest> {
        let rules = self.rules();
        PresetManifest::standard("core", rules.iter().map(|rule| rule.manifest()))
    }
}

/// Fallback provider: every file is plain text.
struct Text {
    manifest: ProviderManifest,
}

impl Text {
    fn new() -> Self {
        Self {
            manifest: ProviderManifest {
                fallback: true,
                capabilities: vec![Capability::Overlays],
                ..ProviderManifest::new("text", vec!["**".to_owned()])
            },
        }
    }
}

impl LanguageProvider for Text {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        let fragments = files
            .iter()
            .map(|source| Fragment {
                files: vec![source.file.clone()],
                ..Fragment::default()
            })
            .collect();
        Ok(Indexed {
            fragments,
            ..Indexed::default()
        })
    }
}

#[derive(Deserialize)]
struct Unconfigured {}

/// A rule about allow annotations. The engine reads the annotations of the
/// whole project and reports these findings itself, because whether an
/// annotation is used depends on every other rule's findings; the rule exists
/// so that configuration, presets and the catalog treat it like any other.
fn annotation_rule(id: &'static str) -> Box<dyn Rule> {
    Box::new(DecisionRule::new(
        id,
        &[],
        |_: &RuleManifest, _: &Ctx, _: Unconfigured| Ok(Vec::new()),
    ))
}
