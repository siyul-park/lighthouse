//! What a project takes from the projects it extends besides their rules: the
//! `injections` and the `plugins`, merged the way ESLint merges the `plugins`
//! of shareable configs.

use super::{Config, InjectionSpec, Projects};
use crate::Error;

impl Config {
    /// This project with what the projects it extends bring: their
    /// `injections`, extended first and in order, where a later entry with
    /// the same `id` replaces the earlier in its place; and the union of their
    /// `plugins`, which follow its own listing, where the project's own entry wins. Resolving again
    /// changes nothing.
    pub fn inheriting(&self, projects: &Projects) -> Result<Self, Error> {
        let chain = self.chain(projects)?;
        let mut merged = self.clone();
        merged.injections = Vec::new();
        for config in &chain {
            for injection in &config.injections {
                put_injection(&mut merged.injections, injection);
            }
        }
        for config in chain.iter().filter(|c| !std::ptr::eq(**c, self)) {
            for plugin in &config.plugins {
                if !merged.lists(&plugin.id) {
                    merged.plugins.push(plugin.clone());
                    merged
                        .required
                        .insert(plugin.id.clone(), config.name.clone());
                }
            }
        }
        merged.resolved = Default::default();
        Ok(merged)
    }

    /// The extended project that required the plugin `id`, when the project
    /// itself does not list it.
    pub fn required_by(&self, id: &str) -> Option<&str> {
        self.required.get(id).map(String::as_str)
    }
}

fn put_injection(into: &mut Vec<InjectionSpec>, injection: &InjectionSpec) {
    match into.iter_mut().find(|i| i.id == injection.id) {
        Some(earlier) => *earlier = injection.clone(),
        None => into.push(injection.clone()),
    }
}
