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

struct Files(LanguageManifest, Arc<AtomicUsize>);

impl LanguageProvider for Files {
    fn manifest(&self) -> &LanguageManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, units: &[Unit], _: &[String]) -> Result<Indexed, PluginError> {
        self.1.fetch_add(1, Ordering::SeqCst);
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
    /// The requests the provider got.
    indexed: Arc<AtomicUsize>,
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
        vec![Box::new(Files(
            LanguageManifest::new("any", vec!["**".to_owned()]),
            Arc::clone(&self.counted.indexed),
        ))]
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
        indexed: Arc::default(),
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
                indexed: Arc::clone(&counted.indexed),
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

    // One file changes: its rule runs again; the one that reads the project
    // does not, for the model of the project is the one it read; the other
    // file's findings are taken.
    fs::write(dir.path().join("a.txt"), "aaaa").unwrap();
    let (third, third_runs) = cached();
    let third = third.check(&[], &[]).unwrap();
    assert_eq!(runs(&third_runs), (1, 0));
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
fn an_edit_and_its_revert_ask_no_provider_and_run_no_rule() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let (engine, counted) = engine(dir.path());
    let engine = engine.with_cache(cache.path().join("cache"), u64::MAX);
    let first = engine.check(&[], &[]).unwrap();
    fs::write(dir.path().join("a.txt"), "aaaa").unwrap();
    engine.check(&[], &[]).unwrap();
    let (asked, ran) = (counted.indexed.load(Ordering::SeqCst), runs(&counted));

    fs::write(dir.path().join("a.txt"), "aa").unwrap();
    let reverted = engine.check(&[], &[]).unwrap();

    assert_eq!(found(&reverted), found(&first));
    assert_eq!(counted.indexed.load(Ordering::SeqCst), asked);
    assert_eq!(runs(&counted), ran);
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

/// Sizes of every file under `dir`.
fn tree(dir: &std::path::Path) -> u64 {
    fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .map(|entry| match entry.metadata() {
                Ok(meta) if meta.is_dir() => tree(&entry.path()),
                Ok(meta) => meta.len(),
                Err(_) => 0,
            })
            .sum()
    })
}

#[test]
fn a_directory_over_its_limit_comes_back_under_it_and_the_findings_are_the_same() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let plugin = kept.join("lang-x");
    fs::create_dir_all(plugin.join("0123456789abcdef")).unwrap();
    fs::write(
        plugin.join("0123456789abcdef/fragments.json"),
        vec![b'x'; 3 << 20],
    )
    .unwrap();
    fs::write(plugin.join("fragments.json"), vec![b'x'; 1 << 20]).unwrap();
    fs::write(plugin.join("initialize-old.json"), "{}").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(plugin.join("initialize-new.json"), "{}").unwrap();
    fs::create_dir_all(kept.join("hooks")).unwrap();
    fs::write(kept.join("hooks/session.json"), "{}").unwrap();

    let (plain, _) = engine(dir.path());
    let plain = plain.check(&[], &[]).unwrap();
    let limit = 1 << 20;
    let (engine, _) = engine(dir.path());
    let cached = engine.with_cache(kept.clone(), limit);
    let cold = cached.check(&[], &[]).unwrap();

    assert!(tree(&kept) <= limit, "{} bytes", tree(&kept));
    assert!(!plugin.join("0123456789abcdef").exists());
    assert!(!plugin.join("fragments.json").exists());
    assert!(!plugin.join("initialize-old.json").exists());
    assert!(plugin.join("initialize-new.json").exists());
    assert!(kept.join("hooks/session.json").exists());
    assert_eq!(found(&cold), found(&plain));
    assert_eq!(found(&cached.check(&[], &[]).unwrap()), found(&plain));
}

#[test]
fn a_limit_that_leaves_nothing_evicts_the_graph_and_results_are_still_the_same() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let (plain, _) = engine(dir.path());
    let plain = plain.check(&[], &[]).unwrap();
    for _ in 0..3 {
        let (engine, _) = engine(dir.path());
        let run = engine.with_cache(kept.clone(), 1).check(&[], &[]).unwrap();
        assert_eq!(found(&run), found(&plain));
    }
    let graph = fs::metadata(kept.join("graph.db")).unwrap().len();
    assert!(graph < 64 << 10, "{graph} bytes");
}

#[test]
fn a_run_under_its_limit_touches_nothing_in_the_directory() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let leftover = kept.join("lang-x/0123456789abcdef");
    fs::create_dir_all(&leftover).unwrap();
    fs::write(leftover.join("fragments.json"), "{}").unwrap();
    let (engine, _) = engine(dir.path());
    engine.with_cache(kept, u64::MAX).check(&[], &[]).unwrap();
    assert!(leftover.join("fragments.json").exists());
}

#[test]
fn an_answer_whose_payload_is_gone_from_the_graph_is_asked_of_the_provider_again() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let (first, _) = engine(dir.path());
    let first = first
        .with_cache(kept.clone(), u64::MAX)
        .check(&[], &[])
        .unwrap();
    {
        let db = rusqlite::Connection::open(kept.join("graph.db")).unwrap();
        db.execute(
            "DELETE FROM blob WHERE hash IN (SELECT hash FROM node WHERE kind = 'payload')",
            [],
        )
        .unwrap();
    }

    let (second, counted) = engine(dir.path());
    let second = second.with_cache(kept, u64::MAX).check(&[], &[]).unwrap();

    assert!(counted.indexed.load(Ordering::SeqCst) > 0, "asked again");
    assert_eq!(found(&second), found(&first));
    assert_eq!(second.incomplete, first.incomplete);
}

#[test]
fn a_run_the_graph_answers_still_brings_the_directory_under_its_limit() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let (engine_a, _) = engine(dir.path());
    let first = engine_a
        .with_cache(kept.clone(), u64::MAX)
        .check(&[], &[])
        .unwrap();
    let before = tree(&kept);

    let (engine_b, counted) = engine(dir.path());
    let warm = engine_b
        .with_cache(kept.clone(), 1)
        .check(&[], &[])
        .unwrap();

    assert_eq!(runs(&counted), (0, 0), "answered from the report");
    assert_eq!(found(&warm), found(&first));
    assert!(tree(&kept) < before, "{} -> {}", before, tree(&kept));
}
