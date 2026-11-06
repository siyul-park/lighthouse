//! The findings of rules that count their runs, as nodes of the graph: what a
//! run takes from the graph, what makes it run again, and what happens when
//! the graph cannot be kept.

use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use lighthouse_engine::{Engine, Run};
use lighthouse_model::{
    Diagnostic, Fingerprint, Options, Position, RunScope, Severity, Span,
    ucm::{Applicability, Fragment},
};
use lighthouse_plugin::{
    Context, Error as PluginError, Fragments, Indexed, LanguageManifest, LanguageProvider, Plugin,
    PluginManifest, Registry, Rule, RuleManifest, Unit, Workspace,
};
use lighthouse_spec::Config;
use tempfile::TempDir;

const CONFIG: &str = "plugins = [\"counting\"]\n[rules]\n\"counting/local\" = \"warn\"\n\"counting/global\" = \"warn\"\n";

struct Files(LanguageManifest);

impl LanguageProvider for Files {
    fn manifest(&self) -> &LanguageManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, units: &[Unit], _: &[String]) -> Result<Indexed, PluginError> {
        Ok(Indexed {
            units: units
                .iter()
                .map(|unit| {
                    Fragments::new(
                        unit.id,
                        unit.files
                            .iter()
                            .map(|source| Fragment {
                                files: vec![source.file.clone()],
                                ..Fragment::default()
                            })
                            .collect(),
                    )
                })
                .collect(),
            ..Indexed::default()
        })
    }
}

/// A rule that counts the times it runs and says what it saw.
struct Counting {
    meta: RuleManifest,
    runs: Arc<AtomicUsize>,
}

impl Rule for Counting {
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }
    fn validate(&self, _: &Options) -> Result<(), PluginError> {
        Ok(())
    }
    fn check(&self, ctx: &Context, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        ctx.notices.push(format!("{} ran", self.meta.id));
        let (file, message) = match ctx.file {
            Some((file, text)) => (file.path.clone(), format!("{} bytes", text.len())),
            None => (
                ctx.model.files().next().unwrap().path.clone(),
                format!("{} files", ctx.model.files().count()),
            ),
        };
        let at = Position { line: 1, col: 1 };
        let fingerprint = Fingerprint::new(&self.meta.id, &file.to_string_lossy(), "");
        Ok(vec![Diagnostic::new(
            &self.meta.id,
            Severity::Warn,
            message,
            file,
            Span { start: at, end: at },
            fingerprint,
        )])
    }
}

/// The rules of the plugin and the counters of their runs.
pub(crate) struct Counted {
    pub(crate) local: Arc<AtomicUsize>,
    global: Arc<AtomicUsize>,
    rows: Arc<AtomicUsize>,
}

struct CountingPlugin {
    manifest: PluginManifest,
    counted: Counted,
}

impl Plugin for CountingPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Files(LanguageManifest::new(
            "any",
            vec!["**".to_owned()],
        )))]
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        let meta = |id: &str, scope| RuleManifest {
            id: id.to_owned(),
            uid: None,
            severity: Severity::Warn,
            scope,
            description: String::new(),
            docs: String::new(),
            analyzers: Vec::new(),
            capabilities: Vec::new(),
            applicability: Applicability::default(),
            revision: Some("1".to_owned()),
            tools: Vec::new(),
        };
        vec![
            Box::new(Counting {
                meta: meta("counting/local", RunScope::File),
                runs: Arc::clone(&self.counted.local),
            }),
            Box::new(Counting {
                meta: meta("counting/global", RunScope::Project),
                runs: Arc::clone(&self.counted.global),
            }),
            Box::new(Counting {
                meta: meta("counting/rows", RunScope::File),
                runs: Arc::clone(&self.counted.rows),
            }),
        ]
    }
}

fn engine(root: &Path) -> (Engine, Counted) {
    engine_with(root, CONFIG)
}

pub(crate) fn engine_with(root: &Path, config: &str) -> (Engine, Counted) {
    let counted = Counted {
        local: Arc::default(),
        global: Arc::default(),
        rows: Arc::default(),
    };
    let mut registry = Registry::default();
    registry
        .register(&CountingPlugin {
            manifest: PluginManifest {
                id: "counting".to_owned(),
                version: "0".to_owned(),
            },
            counted: Counted {
                local: Arc::clone(&counted.local),
                global: Arc::clone(&counted.global),
                rows: Arc::clone(&counted.rows),
            },
        })
        .unwrap();
    let engine = Engine::new(
        registry,
        Config::parse_inline(config).unwrap(),
        lighthouse_test_support::bundled(),
        root,
    )
    .unwrap();
    (engine, counted)
}

fn project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "aa").unwrap();
    fs::write(dir.path().join("b.txt"), "bbb").unwrap();
    dir
}

/// Finds as `(file, message)`, to compare runs by.
fn found(outcome: &Run) -> Vec<(String, String)> {
    outcome
        .diagnostics
        .iter()
        .map(|d| (d.file.display().to_string(), d.message.clone()))
        .collect()
}

fn runs(counted: &Counted) -> (usize, usize) {
    (
        counted.local.load(Ordering::SeqCst),
        counted.global.load(Ordering::SeqCst),
    )
}

#[test]
fn a_run_takes_the_findings_the_graph_has_instead_of_running_rules_again() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let cached = || {
        let (engine, counted) = engine(dir.path());
        (
            engine.with_cache(cache.path().join("cache"), u64::MAX),
            counted,
        )
    };

    let (first, first_runs) = cached();
    let first = first.check(&[], &[]).unwrap();
    assert_eq!(runs(&first_runs), (2, 1));

    let (second, second_runs) = cached();
    let second = second.check(&[], &[]).unwrap();
    assert_eq!(runs(&second_runs), (0, 0));
    assert_eq!(found(&second), found(&first));
    assert_eq!(second.notices, first.notices);
    assert_eq!(
        (second.timings.findings_run, second.timings.findings_green),
        (0, 0),
        "the report of the last run is taken whole"
    );

    // One file changes: its rule runs again and so does the one that reads
    // the project; the other file's findings are taken.
    fs::write(dir.path().join("a.txt"), "aaaa").unwrap();
    let (third, third_runs) = cached();
    let third = third.check(&[], &[]).unwrap();
    assert_eq!(runs(&third_runs), (1, 1));
    let (plain, plain_runs) = engine(dir.path());
    let plain = plain.check(&[], &[]).unwrap();
    assert_eq!(runs(&plain_runs), (2, 1));
    assert_eq!(found(&third), found(&plain));
    assert_eq!(third.notices, plain.notices);

    // The change undone: what the rules saw before is what they see, and
    // the findings are the ones of the first run.
    fs::write(dir.path().join("a.txt"), "aa").unwrap();
    let (fourth, _) = cached();
    let fourth = fourth.check(&[], &[]).unwrap();
    assert_eq!(found(&fourth), found(&first));
}

#[test]
fn a_run_without_a_kept_graph_runs_every_rule() {
    let dir = project();
    let (engine, counted) = engine(dir.path());
    engine.check(&[], &[]).unwrap();
    assert_eq!(runs(&counted), (2, 1));
}

#[test]
fn a_rule_that_reads_the_project_runs_for_every_file_when_the_project_changes() {
    const ROWS: &str = "plugins = [\"counting\"]\n[rules]\n\"counting/rows\" = \"warn\"\n";
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let cached = || {
        let (engine, counted) = engine_with(dir.path(), ROWS);
        (
            engine.with_cache(cache.path().join("cache"), u64::MAX),
            counted,
        )
    };
    let (first, first_runs) = cached();
    let first = first.check(&[], &[]).unwrap();
    assert_eq!(first_runs.rows.load(Ordering::SeqCst), 2);

    let (second, second_runs) = cached();
    let second = second.check(&[], &[]).unwrap();
    assert_eq!(second_runs.rows.load(Ordering::SeqCst), 0);
    assert_eq!(found(&second), found(&first));

    fs::write(dir.path().join("b.txt"), "bbbb").unwrap();
    let (third, _) = cached();
    let third = third.check(&[], &[]).unwrap();
    let (plain, _) = engine_with(dir.path(), ROWS);
    assert_eq!(found(&third), found(&plain.check(&[], &[]).unwrap()));
}
