mod complexity;
mod coupling;
mod doc;
mod wrapper;

use std::path::Path;

use lighthouse_config::{RuleConfig, Rules};
use lighthouse_model::{Diagnostic, Fingerprint, Options, Symbol, SymbolKind};
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
        vec![
            complexity::rule(),
            coupling::rule(),
            doc::rule(),
            wrapper::rule(),
        ]
    }

    fn presets(&self) -> Vec<Preset> {
        let rules = self
            .rules()
            .into_iter()
            .map(|r| r.meta().clone())
            .map(|meta| {
                let config = RuleConfig {
                    level: Some(meta.severity),
                    options: Options::new(),
                };
                (meta.id, config)
            });
        vec![Preset {
            id: "design/recommended".to_owned(),
            rules: Rules::from_iter(rules),
        }]
    }
}

/// Whether the focused file is a test or generated and rules skip it.
fn skipped(ctx: &Ctx) -> bool {
    ctx.file.is_none_or(|(file, _)| {
        file.test || ctx.project.file(&file.path).is_none_or(|f| f.generated)
    })
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
