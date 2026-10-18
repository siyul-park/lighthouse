//! What counts as a finding a fix introduced: more error or warn findings of
//! a rule in a file, wherever in the file they are.

use lighthouse_engine::{Engine, FixRun};
use lighthouse_model::{Diagnostic, Options, Safety};
use lighthouse_plugin::{
    Context, Error as PluginError, Fixer, Plugin, PluginManifest, Registry, Rule, RuleManifest,
};
use lighthouse_spec::{Catalog, Config};

use crate::support::*;

/// Flags `WARN`, each as a finding of the symbol (`fn` line) it follows.
struct Owned(Word);

impl Rule for Owned {
    fn manifest(&self) -> &RuleManifest {
        self.0.manifest()
    }

    fn validate(&self, options: &Options) -> Result<(), PluginError> {
        self.0.validate(options)
    }

    fn check(&self, ctx: &Context, options: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let (_, text) = ctx
            .file
            .ok_or_else(|| PluginError::Failed("no file".into()))?;
        let mut found = self.0.check(ctx, options)?;
        for d in &mut found {
            let before = text.lines().take(d.span.start.line as usize);
            d.symbol = before
                .filter_map(|line| line.strip_prefix("fn "))
                .last()
                .map(str::to_owned);
        }
        Ok(found)
    }
}

/// `judged/move` puts the `TODO` it fixes where a new symbol starts, so the
/// `WARN` after it belongs to that symbol; `judged/grow` makes another `WARN`.
struct Judged(PluginManifest);

impl Plugin for Judged {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![Box::new(Owned(Word::new("judged/warn", "WARN")))]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![
            Box::new(rewrite("judged/move", "fn b\n", Safety::Safe, &[])),
            Box::new(rewrite("judged/grow", "WARN", Safety::Safe, &[])),
        ]
    }
}

fn judged(text: &str) -> (tempfile::TempDir, Engine) {
    let dir = project(text);
    let mut registry = Registry::default();
    registry.register(&Fake(PLUGIN.clone())).unwrap();
    registry
        .register(&Judged(PluginManifest {
            id: "judged".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    let config = Config::parse_inline(
        "plugins = [\"fake\", \"judged\"]\n[rules]\n\"fake/todo\" = \"error\"\n\"judged/warn\" = \"warn\"\n",
    )
    .unwrap();
    let engine = Engine::new(registry, config, Catalog::bundled(), dir.path()).unwrap();
    (dir, engine)
}

#[test]
fn a_finding_that_moves_to_another_symbol_of_the_file_is_not_new() {
    let (dir, engine) = judged("fn a\nTODO WARN\n");

    let report = engine
        .fix(&plan("judged/move", Safety::Safe, true), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\nfn b\n WARN\n", "{:?}", report.declined);
    assert_eq!(report.applied.len(), 1);
    assert!(report.declined.is_empty(), "{:?}", report.declined);
}

#[test]
fn a_fix_that_adds_a_finding_of_a_rule_to_the_file_is_rolled_back() {
    let (dir, engine) = judged("fn a\nTODO WARN\nfn b\n");

    let report = engine
        .fix(&plan("judged/grow", Safety::Safe, true), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\nTODO WARN\nfn b\n");
    let reason = &report.declined[0].reason;
    assert!(reason.contains("judged/warn"), "{reason}");
}
