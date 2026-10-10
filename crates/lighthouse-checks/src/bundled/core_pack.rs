//! What only code provides for the `core` pack: the fallback text language
//! and the rules about allow annotations.

use lighthouse_model::{
    Capability, Fragment,
    annotation::{ANNOTATION_REASON, UNUSED_ALLOW},
};
use lighthouse_plugin::{
    Ctx, Error, Indexed, LanguageProvider, ProviderManifest, Rule, RuleManifest, Source, Workspace,
};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;

/// The language providers of the pack: the fallback text one.
pub(super) fn languages() -> Vec<Box<dyn LanguageProvider>> {
    vec![Box::new(Text::new())]
}

/// The rules about allow annotations.
pub(super) fn annotation_rules() -> Vec<Box<dyn Rule>> {
    vec![
        annotation_rule(ANNOTATION_REASON),
        annotation_rule(UNUSED_ALLOW),
    ]
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
