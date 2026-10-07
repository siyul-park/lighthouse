mod banner;
mod callers;
mod complexity;
mod coupling;
mod doc;
mod helpers;
mod layout;
mod order;
mod qualifier;
mod receiver;
mod related;
mod wrapper;

use std::{path::Path, sync::LazyLock};

use lighthouse_model::{Diagnostic, Fingerprint, Symbol, SymbolKind};
use lighthouse_plugin::{
    Ctx, Fixer, OrderKey, Plugin, PluginManifest, PresetManifest, Rule, RuleManifest,
};
use serde_json::Value;

/// Id of the plugin and prefix of every rule it provides.
pub const ID: &str = "design";

/// The design rules plugin: the rules of the bundled `design` pack.
pub struct Design;

impl Plugin for Design {
    /// The plugin id with this crate's version.
    fn manifest(&self) -> &PluginManifest {
        static MANIFEST: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        &MANIFEST
    }

    /// Every rule of the pack, native and declarative.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        let mut rules = vec![
            banner::rule(),
            callers::rule(),
            complexity::rule(),
            coupling::rule(),
            doc::rule(),
            helpers::rule(),
            order::rule(),
            qualifier::rule(),
            receiver::rule(),
            related::rule(),
            wrapper::rule(),
        ];
        rules.extend(lighthouse_declarative::Declarative::bundled_rules(ID));
        rules
    }

    /// The fixes of the pack's patterns, compiled from the catalog.
    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        lighthouse_declarative::Declarative::bundled_fixers(ID)
    }

    /// The keys `reorder` fixes sort by.
    fn order_keys(&self) -> Vec<Box<dyn OrderKey>> {
        vec![Box::new(order::GroupKey)]
    }

    /// The standard presets over the pack's rules.
    fn presets(&self) -> Vec<PresetManifest> {
        let rules = self.rules();
        PresetManifest::standard(ID, rules.iter().map(|rule| rule.manifest()))
    }
}

/// Whether the focused file is a test or generated and rules skip it.
fn skipped(ctx: &Ctx) -> bool {
    ctx.file.is_none_or(|(file, _)| file.test) || generated(ctx)
}

/// Whether the focused file is generated and rules skip it.
fn generated(ctx: &Ctx) -> bool {
    ctx.file
        .is_none_or(|(file, _)| ctx.project.file(&file.path).is_none_or(|f| f.generated))
}

/// Functions and methods with a body declared in the focused file, outside
/// test modules.
fn functions<'a>(ctx: &Ctx<'a>) -> Vec<&'a Symbol> {
    let Some((file, _)) = ctx.file else {
        return Vec::new();
    };
    let path: &Path = &file.path;
    ctx.project
        .symbols_in(path)
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .filter(|s| ctx.project.function(&s.id).is_some())
        .filter(|s| !ctx.project.in_test(&s.id))
        .collect()
}

fn finding(meta: &RuleManifest, symbol: &Symbol, message: String, evidence: Value) -> Diagnostic {
    let fingerprint = Fingerprint::of(&meta.id, symbol.id.as_str(), "");
    let mut diagnostic = Diagnostic::new(
        &meta.id,
        meta.severity,
        message,
        &symbol.file,
        symbol.span,
        fingerprint,
    );
    diagnostic.symbol = Some(symbol.id.as_str().to_owned());
    diagnostic.evidence = evidence;
    diagnostic
}
