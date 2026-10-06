mod banner;
mod callers;
mod complexity;
mod coupling;
mod doc;
mod helpers;
mod layout;
mod order;
mod qualifier;
mod related;
mod wrapper;

use std::path::Path;

use lighthouse_model::{Diagnostic, Fingerprint, Symbol, SymbolKind};
use lighthouse_plugin::{Ctx, Manifest, Plugin, Preset, Rule, RuleMeta};
use serde_json::Value;

pub const ID: &str = "design";

pub struct Design;

impl Plugin for Design {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

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
            related::rule(),
            wrapper::rule(),
        ];
        rules.extend(lighthouse_declarative::Declarative::bundled_rules(ID));
        rules
    }

    fn presets(&self) -> Vec<Preset> {
        let rules = self.rules();
        Preset::standard(ID, rules.iter().map(|rule| rule.meta()))
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

fn finding(meta: &RuleMeta, symbol: &Symbol, message: String, evidence: Value) -> Diagnostic {
    let fingerprint = Fingerprint::of(&meta.id, symbol.id.as_str(), "");
    let mut diagnostic = Diagnostic::new(
        &meta.id,
        meta.severity,
        message,
        &symbol.file,
        symbol.span,
        fingerprint,
    );
    diagnostic.evidence = evidence;
    diagnostic
}
