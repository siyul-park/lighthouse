//! Writing a verified fix: whole, or not at all.

use lighthouse_spec::Catalog;
use std::fs;

use lighthouse_engine::{Engine, FixBinding, FixPlan, FixRun};
use lighthouse_model::{EditOp, FixOutcome, Safety};
use lighthouse_plugin::{
    Error as PluginError, FixDecision, FixRequest, Fixer, FixerManifest, LanguageProvider, Plugin,
    PluginManifest, Registry, Rule,
};
use lighthouse_spec::Config;

use crate::support::*;

/// A fixer that rewrites TODO in `a.toy` and also appends a line to `b.toy`:
/// one fix over two files.
struct Both(FixerManifest);

impl Fixer for Both {
    fn manifest(&self) -> &FixerManifest {
        &self.0
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        let end = |text: &str| {
            let lines = u32::try_from(text.lines().count()).unwrap() + 1;
            let at = lighthouse_model::Position {
                line: lines,
                col: 1,
            };
            lighthouse_model::Span { start: at, end: at }
        };
        let b = fs::read_to_string(request.ws.root.join("b.toy")).unwrap();
        Ok(FixOutcome::Proposed {
            description: "both".to_owned(),
            ops: vec![
                EditOp::Replace {
                    file: request.finding.file.clone(),
                    span: request.finding.span,
                    text: "DONE".to_owned(),
                },
                EditOp::Replace {
                    file: "b.toy".into(),
                    span: end(&b),
                    text: "// fixed together\n".to_owned(),
                },
            ],
            safety: Safety::Safe,
        })
    }
}

struct Pair(PluginManifest);

impl Plugin for Pair {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Toy(toy()))]
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![Box::new(Word::new("pair/todo", "TODO"))]
    }
    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![Box::new(Both(FixerManifest {
            id: "pair/both".to_owned(),
            requires: Vec::new(),
        }))]
    }
}

fn pair(dir: &tempfile::TempDir, formatter: &str) -> (Engine, FixPlan) {
    let mut registry = Registry::default();
    registry
        .register(&Pair(PluginManifest {
            id: "pair".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    let config = Config::parse_inline(&format!(
        "plugins = [\"pair\"]\n[rules]\n\"pair/todo\" = \"error\"\n{formatter}"
    ))
    .unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "pair/todo",
        FixBinding {
            fixer: "pair/both".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            decision: FixDecision {
                id: "pair/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );
    (
        Engine::new(registry, config, Catalog::bundled(), dir.path()).unwrap(),
        plan,
    )
}

fn two_files() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.toy"), "TODO\n").unwrap();
    fs::write(dir.path().join("b.toy"), "fn b\n").unwrap();
    dir
}

#[test]
fn a_fix_over_two_files_is_written_whole() {
    let dir = two_files();
    let (engine, plan) = pair(&dir, "");

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("a.toy")).unwrap(),
        "DONE\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.toy")).unwrap(),
        "fn b\n// fixed together\n"
    );
    assert_eq!(report.applied.len(), 1);
}

#[test]
fn when_one_file_of_a_fix_changed_concurrently_neither_is_written() {
    let dir = two_files();
    let b = dir.path().join("b.toy");
    // The formatter of a trusted project saves `b.toy` elsewhere while the run
    // is verifying, only when it formats that file.
    let formatter = format!(
        "[languages.toy]\nformatter = [\"sh\", \"-c\", \"case \\\"$0\\\" in *b.toy) echo '// saved elsewhere' >> '{}';; esac\"]\n",
        b.display()
    );
    let (engine, plan) = pair(&dir, &formatter);
    let run = FixRun {
        trusted: true,
        ..FixRun::default()
    };

    let report = engine.fix(&plan, &run).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("a.toy")).unwrap(),
        "TODO\n",
        "a is untouched"
    );
    assert_eq!(
        fs::read_to_string(&b).unwrap(),
        "fn b\n// saved elsewhere\n"
    );
    assert!(report.applied.is_empty() && report.changes.is_empty());
    assert!(
        report.declined[0].reason.contains("changed concurrently"),
        "{:?}",
        report.declined
    );
}

#[cfg(unix)]
#[test]
fn a_write_that_fails_midway_puts_the_written_files_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.toy"), "TODO\n").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/b.toy"), "fn b\n").unwrap();
    // `sub` cannot take the temporary file an atomic write needs.
    fs::set_permissions(dir.path().join("sub"), fs::Permissions::from_mode(0o555)).unwrap();
    let probe = dir.path().join("sub/probe");
    if fs::write(&probe, "x").is_ok() {
        // Running as a user the mode does not bind (root): nothing to test.
        fs::set_permissions(dir.path().join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    struct Sub(FixerManifest);
    impl Fixer for Sub {
        fn manifest(&self) -> &FixerManifest {
            &self.0
        }
        fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
            let at = lighthouse_model::Position { line: 2, col: 1 };
            Ok(FixOutcome::Proposed {
                description: "sub".to_owned(),
                ops: vec![
                    EditOp::Replace {
                        file: request.finding.file.clone(),
                        span: request.finding.span,
                        text: "DONE".to_owned(),
                    },
                    EditOp::Replace {
                        file: "sub/b.toy".into(),
                        span: lighthouse_model::Span { start: at, end: at },
                        text: "// x\n".to_owned(),
                    },
                ],
                safety: Safety::Safe,
            })
        }
    }
    struct Nested(PluginManifest);
    impl Plugin for Nested {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Word::new("nested/todo", "TODO"))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Sub(FixerManifest {
                id: "nested/todo".to_owned(),
                requires: Vec::new(),
            }))]
        }
    }
    let mut registry = Registry::default();
    registry
        .register(&Nested(PluginManifest {
            id: "nested".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    let config =
        Config::parse_inline("plugins = [\"nested\"]\n[rules]\n\"nested/todo\" = \"error\"\n")
            .unwrap();
    let engine = Engine::new(registry, config, Catalog::bundled(), dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "nested/todo",
        FixBinding {
            fixer: "nested/todo".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            decision: FixDecision {
                id: "nested/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    fs::set_permissions(dir.path().join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("a.toy")).unwrap(),
        "TODO\n",
        "a was put back"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("sub/b.toy")).unwrap(),
        "fn b\n"
    );
    assert!(report.applied.is_empty() && report.changes.is_empty());
    assert!(
        report.declined[0].reason.contains("not written"),
        "{:?}",
        report.declined
    );
}

#[test]
fn a_file_whose_provider_does_not_analyze_overlays_is_not_fixed() {
    struct Plain(PluginManifest);
    impl Plugin for Plain {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(lighthouse_plugin::ProviderManifest::new(
                "toy",
                vec!["**/*.toy".to_owned()],
            )))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Word::new("plain/todo", "TODO"))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Both(FixerManifest {
                id: "plain/todo".to_owned(),
                requires: Vec::new(),
            }))]
        }
    }
    let dir = two_files();
    let mut registry = Registry::default();
    registry
        .register(&Plain(PluginManifest {
            id: "plain".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    let config =
        Config::parse_inline("plugins = [\"plain\"]\n[rules]\n\"plain/todo\" = \"error\"\n")
            .unwrap();
    let engine = Engine::new(registry, config, Catalog::bundled(), dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "plain/todo",
        FixBinding {
            fixer: "plain/todo".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            decision: FixDecision {
                id: "plain/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("a.toy")).unwrap(),
        "TODO\n"
    );
    assert!(
        report.declined[0]
            .reason
            .contains("does not analyze overlays"),
        "{:?}",
        report.declined
    );
}

#[test]
fn a_formatter_is_told_the_real_path_and_finds_the_settings_next_to_its_copy() {
    let dir = project("TODO\n");
    fs::write(dir.path().join(".editorconfig"), "root = true\n").unwrap();
    let by_path = "[languages.toy]\nformatter = { argv = [\"sh\", \"-c\", \"cat; echo \\\"// $0\\\"\", \"{path}\"], stdin = \"file\", output = \"text\" }\n";
    let run = FixRun {
        trusted: true,
        ..FixRun::default()
    };

    engine(&dir, by_path)
        .fix(&plan("fake/to-done", Safety::Safe, true), &run)
        .unwrap();

    assert_eq!(text(&dir), "DONE\n// a.toy\n");

    fs::write(dir.path().join("a.toy"), "TODO\n").unwrap();
    let by_copy = "[languages.toy]\nformatter = [\"sh\", \"-c\", \"cat \\\"$(dirname \\\"$0\\\")/.editorconfig\\\" > \\\"$0\\\"\"]\n";

    engine(&dir, by_copy)
        .fix(&plan("fake/to-done", Safety::Safe, true), &run)
        .unwrap();

    assert_eq!(
        text(&dir),
        "root = true\n",
        "the settings were in the scratch tree"
    );
}
