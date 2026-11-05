//! What only code provides for the `core` pack: the fallback text language,
//! and the generated-code markers its manifest declares.

use lighthouse_model::{
    GeneratedSpec,
    ucm::{Capability, Fragment},
};
use lighthouse_plugin::{
    Error, Fragments, Indexed, LanguageManifest, LanguageProvider, Unit, Workspace,
};

/// The manifest of the `core` plugin, which is catalog data: the markers it
/// declares are the defaults of every project that lists the plugin.
const MANIFEST: &str = include_str!("../../../../plugins/core/lighthouse-plugin.toml");

/// Fallback provider: every file is plain text.
struct Text {
    manifest: LanguageManifest,
}

impl Text {
    fn new() -> Self {
        Self {
            manifest: LanguageManifest {
                fallback: true,
                capabilities: vec![Capability::Overlays],
                ..LanguageManifest::new("text", vec!["**".to_owned()])
            },
        }
    }
}

impl LanguageProvider for Text {
    fn manifest(&self) -> &LanguageManifest {
        &self.manifest
    }

    fn index(&self, _: &Workspace, units: &[Unit], _: &[String]) -> Result<Indexed, Error> {
        let units = units
            .iter()
            .map(|unit| {
                let fragments = unit
                    .files
                    .iter()
                    .map(|source| Fragment {
                        files: vec![source.file.clone()],
                        ..Fragment::default()
                    })
                    .collect();
                Fragments::new(unit.id, fragments)
            })
            .collect();
        Ok(Indexed {
            units,
            ..Indexed::default()
        })
    }
}

/// The language providers of the pack: the fallback text one.
pub(crate) fn languages() -> Vec<Box<dyn LanguageProvider>> {
    vec![Box::new(Text::new())]
}

/// The `[spec.generated]` of the manifest of the `core` plugin.
pub(crate) fn generated() -> GeneratedSpec {
    let manifest: toml::Table = toml::from_str(MANIFEST).expect("the core manifest is TOML");
    manifest
        .get("spec")
        .and_then(|spec| spec.get("generated"))
        .cloned()
        .map(|generated| {
            generated
                .try_into()
                .expect("the core markers are a `generated`")
        })
        .unwrap_or_default()
}
