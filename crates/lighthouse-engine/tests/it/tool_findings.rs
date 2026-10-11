//! What a wrapped tool says besides its findings: its own suppressions, and
//! the notices a rule pushes.

use lighthouse_engine::Engine;
use lighthouse_model::{
    Applicability, Diagnostic, Fingerprint, Options, Position, RunScope, Severity, Span,
    Suppression, SuppressionKind,
};
use lighthouse_plugin::{
    Ctx, Error as PluginError, LanguageProvider, Plugin, PluginManifest, Registry, Rule,
    RuleManifest,
};
use lighthouse_spec::{Catalog, Config};

use crate::support::{Toy, project, toy};

/// Reports one finding the tool suppressed itself and one it did not, and
/// says what it left out.
struct Wrapped(RuleManifest);

impl Rule for Wrapped {
    fn manifest(&self) -> &RuleManifest {
        &self.0
    }

    fn validate(&self, _: &Options) -> Result<(), PluginError> {
        Ok(())
    }

    fn check(&self, ctx: &Ctx, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let (file, _) = ctx.file.expect("a file rule");
        ctx.notices.push("tool: 2 result(s) were dropped");
        let at = Position { line: 1, col: 1 };
        let finding = |message: &str, n: usize| {
            Diagnostic::new(
                &self.0.id,
                Severity::Error,
                message,
                &file.path,
                Span { start: at, end: at },
                Fingerprint::of(&self.0.id, "f", message).occurrence(n),
            )
        };
        let mut waived = finding("waived by the tool", 0);
        waived.suppression = Some(Suppression::in_source("known"));
        Ok(vec![waived, finding("open", 1)])
    }
}

struct Tool(PluginManifest);

impl Plugin for Tool {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Toy(toy()))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![Box::new(Wrapped(RuleManifest {
            id: "tool/wrapped".to_owned(),
            uid: None,
            severity: Severity::Error,
            scope: RunScope::File,
            description: String::new(),
            docs: String::new(),
            analyzers: Vec::new(),
            capabilities: Vec::new(),
            applicability: Applicability::default(),
            caching: None,
        }))]
    }
}

#[test]
fn a_finding_the_tool_suppressed_is_reported_suppressed_and_its_notices_reach_the_run() {
    let dir = project("fn a\n");
    let mut registry = Registry::default();
    let manifest = PluginManifest {
        id: "tool".to_owned(),
        version: "0".to_owned(),
    };
    registry.register(&Tool(manifest)).unwrap();
    let config =
        Config::parse_inline("plugins = [\"tool\"]\n[rules]\n\"tool/wrapped\" = \"error\"\n")
            .unwrap();
    let engine = Engine::new(registry, config, Catalog::bundled(), dir.path()).unwrap();

    let out = engine.check(&[], &[]).unwrap();

    let open: Vec<&str> = out.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(open, ["open"]);
    assert_eq!(out.suppressed.len(), 1);
    assert_eq!(out.suppressed[0].diagnostic.message, "waived by the tool");
    assert_eq!(
        out.suppressed[0].suppression.kind,
        SuppressionKind::InSource
    );
    assert_eq!(out.suppressed[0].suppression.justification, "known");
    assert!(out.notices.contains("tool: 2 result(s) were dropped"));
}
