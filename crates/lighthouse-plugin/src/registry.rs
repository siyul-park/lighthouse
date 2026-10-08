use std::collections::{BTreeMap, BTreeSet};

use lighthouse_config::Rules;

use crate::{
    Analyzer, Error, Fixer, LanguageProvider, OrderKey, OrderKeys, Plugin, PluginManifest,
    PresetManifest, Rule,
};

/// The registered plugins and everything they contribute. `register` is atomic,
/// so a registry never holds a half-registered plugin.
#[derive(Default)]
pub struct Registry {
    plugins: Vec<PluginManifest>,
    languages: Vec<(String, Box<dyn LanguageProvider>)>,
    analyzers: BTreeMap<String, Box<dyn Analyzer>>,
    rules: BTreeMap<String, Box<dyn Rule>>,
    presets: BTreeMap<String, PresetManifest>,
    fixers: BTreeMap<String, Box<dyn Fixer>>,
    keys: BTreeMap<String, Box<dyn OrderKey>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Visiting,
    Done,
}

impl Registry {
    /// Registers atomically: on error nothing from `plugin` is kept.
    pub fn register(&mut self, plugin: &dyn Plugin) -> Result<(), Error> {
        let manifest = plugin.manifest().clone();
        if self.has_plugin(&manifest.id) {
            return Err(Error::Duplicate(manifest.id));
        }
        let languages = plugin.languages();
        let analyzers = plugin.analyzers();
        let rules = plugin.rules();
        let presets = plugin.presets();
        let fixers = plugin.fixers();
        let keys = plugin.order_keys();
        self.check_languages(&languages)?;
        check_prefixes(&manifest.id, &analyzers, &rules, &presets)?;
        self.check_unique(&analyzers, &rules, &presets)?;
        self.check_fixers(&manifest.id, &fixers, &keys)?;

        let id = manifest.id.clone();
        self.plugins.push(manifest);
        self.languages
            .extend(languages.into_iter().map(|l| (id.clone(), l)));
        self.analyzers
            .extend(analyzers.into_iter().map(|a| (a.manifest().id.clone(), a)));
        self.rules
            .extend(rules.into_iter().map(|r| (r.manifest().id.clone(), r)));
        self.presets
            .extend(presets.into_iter().map(|p| (p.id.clone(), p)));
        self.fixers
            .extend(fixers.into_iter().map(|f| (f.manifest().id.clone(), f)));
        self.keys
            .extend(keys.into_iter().map(|k| (k.manifest().id.clone(), k)));
        Ok(())
    }

    /// Fixer and order key ids are qualified with the plugin and unique.
    fn check_fixers(
        &self,
        plugin: &str,
        fixers: &[Box<dyn Fixer>],
        keys: &[Box<dyn OrderKey>],
    ) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        for f in fixers {
            let id = f.manifest().id.as_str();
            if plugin_of(id) != plugin || !id.contains('/') {
                return Err(Error::Prefix {
                    plugin: plugin.to_owned(),
                    id: id.to_owned(),
                });
            }
            if self.fixers.contains_key(id) || !seen.insert(id) {
                return Err(Error::Duplicate(id.to_owned()));
            }
        }
        let mut seen = BTreeSet::new();
        for k in keys {
            let id = k.manifest().id.as_str();
            if plugin_of(id) != plugin || !id.contains('/') {
                return Err(Error::Prefix {
                    plugin: plugin.to_owned(),
                    id: id.to_owned(),
                });
            }
            if self.keys.contains_key(id) || !seen.insert(id) {
                return Err(Error::Duplicate(id.to_owned()));
            }
        }
        Ok(())
    }

    /// The fixer with this qualified id.
    pub fn fixer(&self, id: &str) -> Option<&dyn Fixer> {
        self.fixers.get(id).map(Box::as_ref)
    }

    /// Sorted by id.
    pub fn fixers(&self) -> impl Iterator<Item = &dyn Fixer> {
        self.fixers.values().map(Box::as_ref)
    }

    /// The order keys, sorted by id.
    pub fn order_keys(&self) -> impl Iterator<Item = &dyn OrderKey> {
        self.keys.values().map(Box::as_ref)
    }

    fn check_languages(&self, languages: &[Box<dyn LanguageProvider>]) -> Result<(), Error> {
        for l in languages {
            if self
                .languages
                .iter()
                .any(|(_, x)| x.manifest().id == l.manifest().id)
            {
                return Err(Error::Duplicate(l.manifest().id.clone()));
            }
        }
        Ok(())
    }

    /// No id may exist already or repeat within the plugin.
    fn check_unique(
        &self,
        analyzers: &[Box<dyn Analyzer>],
        rules: &[Box<dyn Rule>],
        presets: &[PresetManifest],
    ) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        for a in analyzers {
            let id = a.manifest().id.as_str();
            if self.analyzers.contains_key(id) || !seen.insert(id) {
                return Err(Error::Duplicate(id.to_owned()));
            }
        }
        for r in rules {
            let rule = r.manifest().id.as_str();
            if self.rules.contains_key(rule) || !seen.insert(rule) {
                return Err(Error::Duplicate(rule.to_owned()));
            }
        }
        for p in presets {
            if self.presets.contains_key(&p.id) || !seen.insert(p.id.as_str()) {
                return Err(Error::Duplicate(p.id.clone()));
            }
        }
        Ok(())
    }

    /// Ids of registered plugins, in registration order.
    pub fn plugins(&self) -> impl Iterator<Item = &str> {
        self.plugins.iter().map(|m| m.id.as_str())
    }

    /// Whether a plugin with this id is registered.
    pub fn has_plugin(&self, id: &str) -> bool {
        self.plugins.iter().any(|m| m.id == id)
    }

    /// With the owning plugin id: regular providers by descending priority
    /// (registration order among equals), then fallback providers; the first
    /// provider matching a file wins.
    pub fn languages(&self) -> impl Iterator<Item = (&str, &dyn LanguageProvider)> {
        let (mut regular, fallback): (Vec<_>, Vec<_>) = self
            .languages
            .iter()
            .map(|(p, l)| (p.as_str(), l.as_ref()))
            .partition(|(_, l)| !l.manifest().fallback);
        regular.sort_by_key(|(_, l)| std::cmp::Reverse(l.manifest().priority));
        regular.into_iter().chain(fallback)
    }

    /// The rule with this qualified id.
    pub fn rule(&self, id: &str) -> Option<&dyn Rule> {
        self.rules.get(id).map(Box::as_ref)
    }

    /// Sorted by id.
    pub fn rules(&self) -> impl Iterator<Item = &dyn Rule> {
        self.rules.values().map(Box::as_ref)
    }

    /// The preset with this qualified id.
    pub fn preset(&self, id: &str) -> Option<&PresetManifest> {
        self.presets.get(id)
    }

    /// The rules of the preset with this id, those of the presets it extends
    /// underneath; `None` when it or one it extends is unknown, or when the
    /// presets extend each other in a circle.
    pub fn preset_rules(&self, id: &str) -> Option<Rules> {
        self.flatten(id, &mut Vec::new())
    }

    fn flatten(&self, id: &str, stack: &mut Vec<String>) -> Option<Rules> {
        if stack.iter().any(|seen| seen == id) {
            return None;
        }
        let preset = self.presets.get(id)?;
        stack.push(id.to_owned());
        let mut rules = Rules::new();
        for base in &preset.extends {
            for (rule, config) in self.flatten(base, stack)? {
                rules.insert(rule, config);
            }
        }
        stack.pop();
        rules.extend(preset.rules.clone());
        Some(rules)
    }

    /// `ids` and their transitive requirements, dependencies first.
    pub fn order<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<Vec<&dyn Analyzer>, Error> {
        let roots: BTreeSet<&str> = ids.into_iter().collect();
        let mut marks = BTreeMap::new();
        let mut out = Vec::new();
        for id in roots {
            self.visit(id, "requested", &mut Vec::new(), &mut marks, &mut out)?;
        }
        Ok(out)
    }

    /// Checks every registered analyzer for missing requirements and cycles.
    pub fn validate(&self) -> Result<(), Error> {
        self.order(self.analyzers.keys().map(String::as_str))
            .map(drop)
    }

    fn visit<'r>(
        &'r self,
        id: &str,
        required_by: &str,
        stack: &mut Vec<String>,
        marks: &mut BTreeMap<String, Mark>,
        out: &mut Vec<&'r dyn Analyzer>,
    ) -> Result<(), Error> {
        let analyzer = self
            .analyzers
            .get(id)
            .ok_or_else(|| Error::MissingAnalyzer {
                id: id.to_owned(),
                required_by: required_by.to_owned(),
            })?;
        match marks.get(id) {
            Some(Mark::Done) => return Ok(()),
            Some(Mark::Visiting) => {
                let from = stack.iter().position(|s| s == id).unwrap_or(0);
                let mut cycle = stack[from..].to_vec();
                cycle.push(id.to_owned());
                return Err(Error::Cycle(cycle));
            }
            None => {}
        }
        marks.insert(id.to_owned(), Mark::Visiting);
        stack.push(id.to_owned());
        for dep in &analyzer.manifest().requires {
            self.visit(dep, id, stack, marks, out)?;
        }
        stack.pop();
        marks.insert(id.to_owned(), Mark::Done);
        out.push(analyzer.as_ref());
        Ok(())
    }
}

impl OrderKeys for Registry {
    /// The order key with this qualified id.
    fn key(&self, id: &str) -> Option<&dyn OrderKey> {
        self.keys.get(id).map(Box::as_ref)
    }
}

/// Plugin id of a qualified `plugin/name` id.
pub fn plugin_of(id: &str) -> &str {
    id.split_once('/').map_or(id, |(plugin, _)| plugin)
}

/// Every id a plugin contributes must be qualified with the plugin's own id.
fn check_prefixes(
    plugin: &str,
    analyzers: &[Box<dyn Analyzer>],
    rules: &[Box<dyn Rule>],
    presets: &[PresetManifest],
) -> Result<(), Error> {
    let ids = analyzers
        .iter()
        .map(|a| a.manifest().id.as_str())
        .chain(rules.iter().map(|r| r.manifest().id.as_str()))
        .chain(presets.iter().map(|p| p.id.as_str()));
    for item in ids {
        if plugin_of(item) != plugin || !item.contains('/') {
            return Err(Error::Prefix {
                plugin: plugin.to_owned(),
                id: item.to_owned(),
            });
        }
    }
    Ok(())
}
