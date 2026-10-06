mod assertions;
mod external;
mod layout;
mod naming;
mod owner;
mod single_owner;

use lighthouse_model::{Diagnostic, Fingerprint, Symbol};
use lighthouse_plugin::{Ctx, Manifest, Plugin, Preset, Rule, RuleMeta};
use serde_json::Value;

/// Id of the plugin and prefix of every rule it provides.
pub const ID: &str = "testing";

/// The test-contract rules of the `testing` pattern pack.
pub struct Testing;

impl Plugin for Testing {
    /// The plugin id with this crate's version.
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Every rule of the `testing` pack.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![
            assertions::rule(),
            external::rule(),
            layout::rule(),
            owner::rule(),
            single_owner::rule(),
        ]
    }

    /// The standard presets over the pack's rules.
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
    diagnostic.symbol = Some(symbol.id.as_str().to_owned());
    diagnostic.evidence = evidence;
    diagnostic
}
