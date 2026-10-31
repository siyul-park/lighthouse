use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

use crate::{
    Analyzer, CelCatalog, CelLibrary, Error, Fixer, LanguageManifest, LanguageProvider,
    ModelClient, Plugin, PluginManifest, Rule,
};

/// The analyzers of one dependency walk, dependencies first.
struct Walk<'r> {
    registry: &'r Registry,
    marks: BTreeMap<String, Mark>,
    stack: Vec<String>,
    out: Vec<&'r dyn Analyzer>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Visiting,
    Done,
}

/// The registered plugins and everything they contribute. `register` is atomic,
/// so a registry never holds a half-registered plugin.
#[derive(Default)]
pub struct Registry {
    plugins: Vec<PluginManifest>,
    languages: Vec<Language>,
    analyzers: BTreeMap<String, Box<dyn Analyzer>>,
    rules: BTreeMap<String, Box<dyn Rule>>,
    fixers: BTreeMap<String, Box<dyn Fixer>>,
    models: BTreeMap<String, Box<dyn ModelClient>>,
    libraries: Vec<Box<dyn CelLibrary>>,
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
        let fixers = plugin.fixers();
        let models = plugin.models();
        let libraries = plugin.cel_libraries();
        let id = manifest.id.as_str();
        let own = |model: &str| model == id;
        let qualified = |item: &str| plugin_of(item) == id && item.contains('/');
        let mut seen = BTreeSet::new();
        claim(
            models.iter().map(|m| m.manifest().id.as_str()),
            own,
            |m| self.models.contains_key(m),
            &mut BTreeSet::new(),
            id,
        )?;
        self.check_languages(&languages)?;
        claim(
            analyzers.iter().map(|a| a.manifest().id.as_str()),
            qualified,
            |a| self.analyzers.contains_key(a),
            &mut seen,
            id,
        )?;
        claim(
            rules.iter().map(|r| r.manifest().id.as_str()),
            qualified,
            |r| self.rules.contains_key(r),
            &mut seen,
            id,
        )?;
        claim(
            fixers.iter().map(|f| f.manifest().id.as_str()),
            qualified,
            |f| self.fixers.contains_key(f),
            &mut BTreeSet::new(),
            id,
        )?;

        self.check_libraries(&libraries, id)?;

        let id = manifest.id.clone();
        self.plugins.push(manifest);
        self.languages
            .extend(languages.into_iter().map(|provider| Language {
                plugin: id.clone(),
                provider,
            }));
        // Regular providers by descending priority (registration order among
        // equals), then the fallback ones: the first provider matching a file wins.
        self.languages.sort_by_key(|l| {
            let manifest = l.provider.manifest();
            (
                manifest.fallback,
                Reverse(if manifest.fallback {
                    0
                } else {
                    manifest.priority
                }),
            )
        });
        self.analyzers
            .extend(analyzers.into_iter().map(|a| (a.manifest().id.clone(), a)));
        self.rules
            .extend(rules.into_iter().map(|r| (r.manifest().id.clone(), r)));
        self.fixers
            .extend(fixers.into_iter().map(|f| (f.manifest().id.clone(), f)));
        self.models
            .extend(models.into_iter().map(|m| (m.manifest().id.clone(), m)));
        self.libraries.extend(libraries);
        Ok(())
    }

    /// The CEL libraries of the registered plugins and their function catalog.
    pub fn cel(&self) -> CelCatalog<'_> {
        CelCatalog::new(&self.libraries)
    }

    /// The model with this id.
    pub fn model(&self, id: &str) -> Option<&dyn ModelClient> {
        self.models.get(id).map(Box::as_ref)
    }

    /// The fixer with this qualified id.
    pub fn fixer(&self, id: &str) -> Option<&dyn Fixer> {
        self.fixers.get(id).map(Box::as_ref)
    }

    /// Sorted by id.
    pub fn fixers(&self) -> impl Iterator<Item = &dyn Fixer> {
        self.fixers.values().map(Box::as_ref)
    }

    /// The libraries of a plugin are named after it, and neither a library nor
    /// a function (a name and the kind of value it is called on) is provided
    /// twice, within the plugin or against the registered ones.
    fn check_libraries(
        &self,
        libraries: &[Box<dyn CelLibrary>],
        plugin: &str,
    ) -> Result<(), Error> {
        let mut ids = BTreeSet::new();
        let mut functions: BTreeMap<(&str, Option<&str>), &str> = self
            .cel()
            .functions()
            .into_iter()
            .map(|c| {
                (
                    (c.function.name.as_str(), c.function.receiver.as_deref()),
                    c.library,
                )
            })
            .collect();
        for library in libraries {
            let manifest = library.manifest();
            if plugin_of(&manifest.id) != plugin || !manifest.id.contains('/') {
                return Err(Error::Prefix {
                    plugin: plugin.to_owned(),
                    id: manifest.id.clone(),
                });
            }
            if !ids.insert(manifest.id.as_str())
                || self
                    .libraries
                    .iter()
                    .any(|l| l.manifest().id == manifest.id)
            {
                return Err(Error::Duplicate(manifest.id.clone()));
            }
            for function in &manifest.functions {
                let key = (function.name.as_str(), function.receiver.as_deref());
                if let Some(other) = functions.insert(key, manifest.id.as_str()) {
                    return Err(Error::Failed(format!(
                        "CEL function `{}` is provided by both `{other}` and `{}`",
                        function.name, manifest.id
                    )));
                }
                let mut seen = BTreeSet::new();
                if let Some(o) = function.overloads.iter().find(|o| !seen.insert(&o.params)) {
                    return Err(Error::Failed(format!(
                        "CEL function `{}` of `{}` has the overload ({}) twice",
                        function.name,
                        manifest.id,
                        o.params.join(", ")
                    )));
                }
            }
        }
        Ok(())
    }

    fn check_languages(&self, languages: &[Box<dyn LanguageProvider>]) -> Result<(), Error> {
        let mut claimed: BTreeMap<String, String> = self
            .languages
            .iter()
            .flat_map(|x| aliases_of(x.provider.manifest()))
            .collect();
        let mut ids: BTreeSet<&str> = self
            .languages
            .iter()
            .map(|x| x.provider.manifest().id.as_str())
            .collect();
        for l in languages {
            let manifest = l.manifest();
            if !ids.insert(&manifest.id) {
                return Err(Error::Duplicate(manifest.id.clone()));
            }
            for (alias, id) in aliases_of(manifest) {
                if let Some(first) = claimed.insert(alias.clone(), id.clone())
                    && first != id
                {
                    return Err(Error::Alias {
                        alias,
                        first,
                        second: id,
                    });
                }
            }
        }
        Ok(())
    }

    /// The id of the language a name stands for, without regard to case: one
    /// of the aliases a provider declared.
    pub fn language_named(&self, name: &str) -> Option<&str> {
        let name = name.to_lowercase();
        self.languages
            .iter()
            .map(|l| l.provider.manifest())
            .find(|manifest| manifest.aliases.iter().any(|a| a.to_lowercase() == name))
            .map(|manifest| manifest.id.as_str())
    }

    /// The manifests of the registered plugins, in registration order.
    pub fn manifests(&self) -> &[PluginManifest] {
        &self.plugins
    }

    /// Whether a plugin with this id is registered.
    pub fn has_plugin(&self, id: &str) -> bool {
        self.plugins.iter().any(|m| m.id == id)
    }

    /// Regular providers by descending priority (registration order among
    /// equals), then fallback providers; the first provider matching a file
    /// wins.
    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    /// The rule with this qualified id.
    pub fn rule(&self, id: &str) -> Option<&dyn Rule> {
        self.rules.get(id).map(Box::as_ref)
    }

    /// Sorted by id.
    pub fn rules(&self) -> impl Iterator<Item = &dyn Rule> {
        self.rules.values().map(Box::as_ref)
    }

    /// `ids` and their transitive requirements, dependencies first.
    pub fn order<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<Vec<&dyn Analyzer>, Error> {
        let roots: BTreeSet<&str> = ids.into_iter().collect();
        let mut walk = Walk {
            registry: self,
            marks: BTreeMap::new(),
            stack: Vec::new(),
            out: Vec::new(),
        };
        for id in roots {
            walk.visit(id, "requested")?;
        }
        Ok(walk.out)
    }

    /// Checks every registered analyzer for missing requirements and cycles.
    pub fn validate(&self) -> Result<(), Error> {
        self.order(self.analyzers.keys().map(String::as_str))
            .map(drop)
    }
}

/// A language provider with the id of the plugin that registered it.
pub struct Language {
    pub plugin: String,
    pub provider: Box<dyn LanguageProvider>,
}

impl<'r> Walk<'r> {
    fn visit(&mut self, id: &str, required_by: &str) -> Result<(), Error> {
        let analyzer = self
            .registry
            .analyzers
            .get(id)
            .ok_or_else(|| Error::MissingAnalyzer {
                id: id.to_owned(),
                required_by: required_by.to_owned(),
            })?;
        match self.marks.get(id) {
            Some(Mark::Done) => return Ok(()),
            Some(Mark::Visiting) => {
                let from = self.stack.iter().position(|s| s == id).unwrap_or(0);
                let mut cycle = self.stack[from..].to_vec();
                cycle.push(id.to_owned());
                return Err(Error::Cycle(cycle));
            }
            None => {}
        }
        self.marks.insert(id.to_owned(), Mark::Visiting);
        self.stack.push(id.to_owned());
        for dep in &analyzer.manifest().requires {
            self.visit(dep, id)?;
        }
        self.stack.pop();
        self.marks.insert(id.to_owned(), Mark::Done);
        self.out.push(analyzer.as_ref());
        Ok(())
    }
}

/// Plugin id of a qualified `plugin/name` id.
pub fn plugin_of(id: &str) -> &str {
    id.split_once('/').map_or(id, |(plugin, _)| plugin)
}

/// Every id a plugin contributes is named as `named` demands, and neither
/// `taken` already nor repeated in `seen`.
fn claim<'a>(
    ids: impl Iterator<Item = &'a str>,
    named: impl Fn(&str) -> bool,
    taken: impl Fn(&str) -> bool,
    seen: &mut BTreeSet<&'a str>,
    plugin: &str,
) -> Result<(), Error> {
    for id in ids {
        if !named(id) {
            return Err(Error::Prefix {
                plugin: plugin.to_owned(),
                id: id.to_owned(),
            });
        }
        if taken(id) || !seen.insert(id) {
            return Err(Error::Duplicate(id.to_owned()));
        }
    }
    Ok(())
}

/// The aliases of a provider in lower case, each with the id it names.
fn aliases_of(manifest: &LanguageManifest) -> impl Iterator<Item = (String, String)> + '_ {
    manifest
        .aliases
        .iter()
        .map(|alias| (alias.to_lowercase(), manifest.id.clone()))
}
