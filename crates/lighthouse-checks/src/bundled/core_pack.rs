//! What only code provides for the `core` pack: the fallback text language.

use lighthouse_model::{Capability, Fragment};
use lighthouse_plugin::{Error, Indexed, LanguageProvider, ProviderManifest, Source, Workspace};

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

/// The language providers of the pack: the fallback text one.
pub(super) fn languages() -> Vec<Box<dyn LanguageProvider>> {
    vec![Box::new(Text::new())]
}
