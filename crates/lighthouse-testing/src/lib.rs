mod external;
mod naming;
mod owner;
mod single_owner;

use lighthouse_model::{Diagnostic, Fingerprint, Symbol};
use lighthouse_plugin::{Ctx, Manifest, Plugin, Preset, Rule, RuleMeta};
use serde_json::Value;

pub const ID: &str = "testing";

/// The test-contract rules of the `testing` pattern pack.
pub struct Testing;

impl Plugin for Testing {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![external::rule(), owner::rule(), single_owner::rule()]
    }

    fn presets(&self) -> Vec<Preset> {
        let rules = self.rules();
        Preset::standard("testing", rules.iter().map(|rule| rule.meta()))
    }
}

/// Whether the focused file is generated; rules about production symbols skip
/// it.
fn generated(ctx: &Ctx) -> bool {
    ctx.file
        .is_none_or(|(file, _)| ctx.project.file(&file.path).is_none_or(|f| f.generated))
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
