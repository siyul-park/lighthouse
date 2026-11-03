//! A fix of any kind is verified against the order a file loads in: a fix
//! that leaves code needing a name bound below it is rolled back.

use lighthouse_engine::{Engine, FixRun};
use lighthouse_model::Safety;
use lighthouse_plugin::{Fixer, Plugin, PluginManifest, Registry};
use lighthouse_spec::Config;

use crate::support::*;

/// A plugin whose fixers write `load` lines, a use as the file loads.
struct Loads(PluginManifest);

impl Plugin for Loads {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![
            Box::new(rewrite("loads/below", "load b", Safety::Safe, &[])),
            Box::new(rewrite("loads/above", "load a", Safety::Safe, &[])),
            Box::new(rewrite("loads/call", "call b", Safety::Safe, &[])),
        ]
    }
}

fn loads(text: &str) -> (tempfile::TempDir, Engine) {
    let dir = project(text);
    let mut registry = Registry::default();
    registry.register(&Fake(PLUGIN.clone())).unwrap();
    registry
        .register(&Loads(PluginManifest {
            id: "loads".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    let config = Config::parse_inline(
        "plugins = [\"fake\", \"loads\"]\n[rules]\n\"fake/todo\" = \"error\"\n",
    )
    .unwrap();
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap();
    (dir, engine)
}

#[test]
fn a_fix_that_makes_code_load_a_name_bound_below_it_is_rolled_back() {
    let source = "fn a\nTODO\nfn b\n";
    let (dir, engine) = loads(source);

    let report = engine
        .fix(&plan("loads/below", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), source);
    let reason = &report.declined[0].reason;
    assert!(
        reason.contains("load order") && reason.contains("`a`") && reason.contains("`b`"),
        "{reason}"
    );
}

#[test]
fn a_fix_that_makes_code_load_a_name_bound_above_it_is_kept() {
    let (dir, engine) = loads("fn a\nfn b\nTODO\n");

    engine
        .fix(&plan("loads/above", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\nfn b\nload a\n");
}

#[test]
fn a_fix_that_makes_code_call_a_name_bound_below_it_orders_nothing() {
    let (dir, engine) = loads("fn a\nTODO\nfn b\n");

    engine
        .fix(&plan("loads/call", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\ncall b\nfn b\n");
}

#[test]
fn a_need_the_file_already_broke_does_not_stop_an_unrelated_fix() {
    let (dir, engine) = loads("fn a\nload b\nTODO\nfn b\n");

    engine
        .fix(&plan("loads/above", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\nload b\nload a\nfn b\n");
}
