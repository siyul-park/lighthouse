//! One engine run over the scratch project of all the shapes: the packages
//! each decision flags.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Config, Decision};
use serde_json::{Map, Value};

use super::{Alphabet, Language, Scratch, enumerate};
use crate::{Engine, tester::rules_config};

/// The packages each of these decisions flags, by decision id, from one run of
/// the engine with only them over a scratch project of all the shapes of up
/// to `bound` declarations (`setting`: the language, alphabet and bound). A
/// gap of the analysis, or a rule that did not run, is an error: what was
/// not read says nothing.
pub(super) fn flags(
    catalog: &Catalog,
    registry: Arc<Registry>,
    setting: (Language, Alphabet, usize),
    decisions: &[(&Decision, &Map<String, Value>)],
) -> Result<BTreeMap<String, Vec<usize>>, String> {
    let (language, alphabet, bound) = setting;
    let shapes = enumerate(language, alphabet, bound).map_err(|why| why.to_string())?;
    let scratch = Scratch::render(language, alphabet, &shapes).map_err(|why| why.to_string())?;
    let rules: Vec<(&Decision, Option<&Map<String, Value>>)> = decisions
        .iter()
        .map(|(decision, options)| (*decision, Some(*options)))
        .collect();
    let config = rules_config(catalog, &registry, &rules, &extends(catalog, language))?;
    let ids: Vec<String> = decisions.iter().map(|(d, _)| d.id().to_owned()).collect();
    let engine =
        Engine::new(registry, config, catalog, scratch.root()).map_err(|why| why.to_string())?;
    let run = engine.check(&[], &ids).map_err(|why| why.to_string())?;
    if let Some(gap) = run.incomplete.first() {
        let at = gap
            .path
            .as_ref()
            .map_or(String::new(), |p| format!("{}: ", p.display()));
        return Err(format!(
            "the engine could not analyze the shapes ({} gaps): {at}{}",
            run.incomplete.len(),
            gap.reason
        ));
    }
    if let Some(idle) = ids.iter().find(|id| !run.ran.contains(id)) {
        return Err(format!("the rule `{idle}` did not run over the shapes"));
    }
    let mut flags: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for finding in &run.diagnostics {
        let Some(unit) = unit_of(&scratch, &finding.file) else {
            return Err(format!(
                "finding outside the shapes: {} {}",
                finding.rule_id,
                finding.file.display()
            ));
        };
        flags.entry(finding.rule_id.clone()).or_default().push(unit);
    }
    for units in flags.values_mut() {
        units.sort_unstable();
        units.dedup();
    }
    Ok(flags)
}

/// The shareable projects that inject `language` into the files of a project:
/// what a language is written in is their data, and it decides what the
/// facts of a shape are.
pub(super) fn extends(catalog: &Catalog, language: Language) -> Vec<String> {
    catalog
        .projects()
        .injecting(language.id())
        .map(str::to_owned)
        .collect()
}

/// The plugins that read the shapes and the CEL libraries the checks call,
/// with their versions: what moves when a provider or a library does.
pub(super) fn provider(registry: &Registry) -> String {
    let mut plugins: Vec<String> = registry
        .manifests()
        .iter()
        .map(|plugin| format!("{}@{}", plugin.id, plugin.version))
        .chain(registry.cel().libraries().map(|library| {
            let manifest = library.manifest();
            format!("cel:{}@{}", manifest.id, manifest.version)
        }))
        .collect();
    plugins.sort();
    plugins.join(",")
}

/// The digest of the projects that inject the language, as the engine
/// resolves them: configuration the options of a decision do not hold.
pub(super) fn injected(catalog: &Catalog, language: Language) -> Result<String, String> {
    let extends = extends(catalog, language);
    if extends.is_empty() {
        return Ok(String::new());
    }
    let list = toml::Value::try_from(&extends).map_err(|why| why.to_string())?;
    let config = Config::parse_inline(&format!("extends = {list}"))
        .and_then(|config| config.inheriting(catalog.projects()))
        .map_err(|why| why.to_string())?;
    let hash = blake3::hash(format!("{config:?}").as_bytes());
    Ok(hash.to_hex()[..16].to_owned())
}

/// The package a file of the scratch project belongs to: `p3/p3.go` and
/// `src/p3.rs` are the package `p3`.
fn unit_of(scratch: &Scratch, file: &Path) -> Option<usize> {
    scratch
        .units()
        .iter()
        .position(|unit| unit.files.iter().any(|f| f == file))
}
