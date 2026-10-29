//! How the regions a host file routed to other providers are told apart from
//! the file: which languages it holds, and which of them are example code.
//! The providers that analyze the regions know none of it; the engine states
//! it on the files and modules they return.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_model::ucm::{Fragment, REGIONS};
use lighthouse_plugin::EmbeddedSource;

/// The reserved context key of a source rule that marks its regions as
/// example code.
const EXAMPLE: &str = "example";

/// The regions of each host file, by the language they were routed to: whether
/// every one of them is example code.
#[derive(Default)]
pub(super) struct Views {
    by_path: BTreeMap<PathBuf, BTreeMap<String, bool>>,
}

impl Views {
    /// Notes that `path` has `sources` routed to the provider of `language`.
    pub(super) fn add(&mut self, path: &Path, language: &str, sources: &[EmbeddedSource]) {
        let example = sources.iter().all(is_example);
        self.by_path
            .entry(path.to_owned())
            .or_default()
            .insert(language.to_owned(), example);
    }

    /// The fact that lists the regions of `path`, see `File::regions`.
    pub(super) fn regions(&self, path: &Path) -> Option<String> {
        let views = self.by_path.get(path)?;
        let listed: Vec<String> = views
            .iter()
            .map(|(language, example)| {
                format!("{language}:{}", if *example { "example" } else { "code" })
            })
            .collect();
        Some(listed.join(","))
    }

    /// The language a host file with no code model of its own is analyzed as:
    /// the one language of the code it holds. Example code is not the file's
    /// language, however much of it there is.
    pub(super) fn analysis_language(&self, path: &Path) -> Option<&str> {
        let mut code = self
            .by_path
            .get(path)?
            .iter()
            .filter(|(_, example)| !**example)
            .map(|(language, _)| language.as_str());
        let language = code.next()?;
        code.next().is_none().then_some(language)
    }

    /// States on what the provider of `language` returned for the regions of
    /// its host files what the host knows: the language of the modules they
    /// declare, whether they are example code, and that example code has no
    /// tests to own.
    pub(super) fn stamp(&self, fragments: &mut [Fragment], language: &str) {
        for fragment in fragments {
            let Some(path) = fragment.files.first().map(|file| file.path.clone()) else {
                continue;
            };
            let Some(views) = self.by_path.get(&path) else {
                continue;
            };
            let example = views.get(language).copied().unwrap_or(false);
            for module in &mut fragment.modules {
                module.language = Some(language.to_owned());
                module.example = example;
            }
            if example {
                fragment.tests.clear();
            }
            for file in &mut fragment.files {
                if let Some(regions) = self.regions(&file.path) {
                    file.facts.insert(REGIONS.to_owned(), regions);
                }
            }
        }
    }
}

/// Whether a region is example code, as its source rule says.
pub(super) fn is_example(source: &EmbeddedSource) -> bool {
    source
        .context
        .get(EXAMPLE)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
