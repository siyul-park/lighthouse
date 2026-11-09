//! The findings of rules that count their runs, as nodes of the graph: what a
//! run takes from the graph, what makes it run again, and what happens when
//! the graph cannot be kept.

use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use lighthouse_engine::{Engine, Run};
use lighthouse_model::{
    Diagnostic, Fingerprint, Incomplete, Options, Position, RunScope, Severity, Span,
    ucm::{Applicability, Fragment},
};
use lighthouse_plugin::{
    Context, Error as PluginError, Fragments, Indexed, LanguageManifest, LanguageProvider, Plugin,
    PluginManifest, Registry, Rule, RuleManifest, Unit, Workspace,
};
use lighthouse_spec::Config;
use tempfile::TempDir;

const CONFIG: &str = "plugins = [\"counting\"]\n[rules]\n\"counting/local\" = \"warn\"\n\"counting/global\" = \"warn\"\n";

struct Files(
    LanguageManifest,
    Arc<AtomicUsize>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
);

impl LanguageProvider for Files {
    fn manifest(&self) -> &LanguageManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, units: &[Unit], _: &[String]) -> Result<Indexed, PluginError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        if self.2.load(Ordering::SeqCst) {
            return Err(PluginError::Failed("the provider is down".to_owned()));
        }
        Ok(Indexed {
            units: units
                .iter()
                .map(|unit| {
                    let files = if self.3.load(Ordering::SeqCst) {
                        &[]
                    } else {
                        unit.files
                    };
                    let mut answer = Fragments::new(
                        unit.id,
                        files
                            .iter()
                            .map(|source| {
                                let mut file = source.file.clone();
                                let bulk = self.5.load(Ordering::SeqCst);
                                if bulk > 0 {
                                    file.facts.insert("bulk".to_owned(), noise(&file, bulk));
                                }
                                Fragment {
                                    files: vec![file],
                                    ..Fragment::default()
                                }
                            })
                            .collect(),
                    );
                    if self.4.load(Ordering::SeqCst) {
                        answer.incomplete.push(Incomplete {
                            path: Some(unit.id.into()),
                            provider: None,
                            reason: "cannot be loaded".to_owned(),
                        });
                    }
                    answer
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
    /// Whether a run takes a while, as a rule's does.
    slow: Arc<AtomicBool>,
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
        if self.slow.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        ctx.notices.push(format!("{} ran", self.meta.id));
        let (file, message) = match ctx.file {
            Some((file, text)) => (file.path.clone(), format!("{} bytes", text.len())),
            None => match ctx.model.files().next() {
                Some(first) => (
                    first.path.clone(),
                    format!("{} files", ctx.model.files().count()),
                ),
                None => return Ok(Vec::new()),
            },
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
    /// Whether the provider fails.
    down: Arc<AtomicBool>,
    /// Whether the provider answers with no fragment.
    mute: Arc<AtomicBool>,
    /// Whether the provider says of every file that it cannot be loaded.
    gapped: Arc<AtomicBool>,
    /// The bytes of a fact the provider gives every file, which the model
    /// carries and no rule reads.
    bulk: Arc<AtomicUsize>,
    /// Whether the rules take a while to run.
    slow: Arc<AtomicBool>,
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
            Arc::clone(&self.counted.down),
            Arc::clone(&self.counted.mute),
            Arc::clone(&self.counted.gapped),
            Arc::clone(&self.counted.bulk),
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
                slow: Arc::clone(&self.counted.slow),
            }),
            Box::new(Counting {
                meta: meta("counting/global", RunScope::Project),
                runs: Arc::clone(&self.counted.global),
                slow: Arc::clone(&self.counted.slow),
            }),
            Box::new(Counting {
                meta: meta("counting/rows", RunScope::File),
                runs: Arc::clone(&self.counted.rows),
                slow: Arc::clone(&self.counted.slow),
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
        down: Arc::default(),
        mute: Arc::default(),
        gapped: Arc::default(),
        bulk: Arc::default(),
        slow: Arc::default(),
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
                down: Arc::clone(&counted.down),
                mute: Arc::clone(&counted.mute),
                gapped: Arc::clone(&counted.gapped),
                bulk: Arc::clone(&counted.bulk),
                slow: Arc::clone(&counted.slow),
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

/// Takes the payloads of the graph away, rows and all, as a forgetting
/// that went wrong would.
fn forget_payloads(kept: &Path) {
    let db = rusqlite::Connection::open(kept.join("graph.db")).unwrap();
    db.execute("DELETE FROM node WHERE kind = 'payload'", [])
        .unwrap();
    db.execute(
        "DELETE FROM blob WHERE hash NOT IN (SELECT hash FROM node)",
        [],
    )
    .unwrap();
}

#[test]
fn a_graph_that_lost_its_payloads_gives_the_full_result_and_never_an_empty_one() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let (first, _) = engine(dir.path());
    let first = first
        .with_cache(kept.clone(), u64::MAX)
        .check(&[], &[])
        .unwrap();
    assert!(!first.diagnostics.is_empty());
    forget_payloads(&kept);

    let (second, counted) = engine(dir.path());
    let second = second.with_cache(kept, u64::MAX).check(&[], &[]).unwrap();

    assert!(counted.indexed.load(Ordering::SeqCst) > 0, "asked again");
    assert_eq!(found(&second), found(&first));
    assert_eq!(second.incomplete, first.incomplete);
}

#[test]
fn a_run_that_could_not_have_its_payloads_is_incomplete_and_is_not_the_report() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let (first, _) = engine(dir.path());
    let first = first
        .with_cache(kept.clone(), u64::MAX)
        .check(&[], &[])
        .unwrap();
    forget_payloads(&kept);

    let (down, counted) = engine(dir.path());
    counted.down.store(true, Ordering::SeqCst);
    let down = down
        .with_cache(kept.clone(), u64::MAX)
        .check(&[], &[])
        .unwrap();
    assert!(!down.incomplete.is_empty());
    assert!(down.diagnostics.is_empty());

    let (third, counted) = engine(dir.path());
    let third = third.with_cache(kept, u64::MAX).check(&[], &[]).unwrap();
    assert!(counted.indexed.load(Ordering::SeqCst) > 0, "asked again");
    assert_eq!(found(&third), found(&first));
    assert!(third.incomplete.is_empty());
}

#[test]
fn a_file_the_model_does_not_state_is_a_gap_and_the_run_is_not_the_report() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let (mute, counted) = engine(dir.path());
    counted.mute.store(true, Ordering::SeqCst);
    let mute = mute
        .with_cache(kept.clone(), u64::MAX)
        .check(&[], &[])
        .unwrap();
    let paths: Vec<_> = mute
        .incomplete
        .iter()
        .map(|gap| gap.path.as_deref())
        .collect();
    assert_eq!(
        paths,
        [Some(Path::new("a.txt")), Some(Path::new("b.txt"))],
        "{:?}",
        mute.incomplete
    );
    assert_eq!(mute.incomplete[0].provider.as_deref(), Some("any"));

    // The empty answer is the graph's, and the next run says the same.
    let (again, _) = engine(dir.path());
    let again = again.with_cache(kept, u64::MAX).check(&[], &[]).unwrap();
    assert_eq!(again.incomplete, mute.incomplete);
}

#[test]
fn a_gap_the_provider_gives_in_its_answer_is_kept_with_the_report() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let cached = || {
        let (engine, counted) = engine(dir.path());
        counted.gapped.store(true, Ordering::SeqCst);
        (engine.with_cache(kept.clone(), u64::MAX), counted)
    };

    let (first, _) = cached();
    let first = first.check(&[], &[]).unwrap();
    assert_eq!(first.incomplete.len(), 2, "{:?}", first.incomplete);
    let (second, counted) = cached();
    let second = second.check(&[], &[]).unwrap();

    assert_eq!(runs(&counted), (0, 0), "answered from the report");
    assert_eq!(counted.indexed.load(Ordering::SeqCst), 0);
    assert_eq!(found(&second), found(&first));
    assert_eq!(second.incomplete, first.incomplete);
    let (plain, counted) = engine(dir.path());
    counted.gapped.store(true, Ordering::SeqCst);
    let plain = plain.check(&[], &[]).unwrap();
    assert_eq!(found(&second), found(&plain));
    assert_eq!(second.incomplete, plain.incomplete);
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

/// `len` characters that the file of the cache does not shrink by much, for
/// the file `file`.
fn noise(file: &lighthouse_model::ucm::File, len: usize) -> String {
    let mut state = file
        .path
        .to_string_lossy()
        .bytes()
        .fold(7u32, |state, byte| {
            state.wrapping_mul(31).wrapping_add(u32::from(byte))
        });
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            char::from(33 + ((state >> 24) % 94) as u8)
        })
        .collect()
}

#[test]
fn edits_and_reverts_under_a_limit_the_graph_keeps_to_run_only_the_rules_of_what_changed() {
    const FILES: usize = 24;
    let dir = tempfile::tempdir().unwrap();
    let text = |file: usize, version: usize| format!("{file}/{version}\n");
    for file in 0..FILES {
        fs::write(dir.path().join(format!("f{file}.txt")), text(file, 0)).unwrap();
    }
    let cache = tempfile::tempdir().unwrap();
    let kept = cache.path().join("cache");
    let cached = |limit| {
        let (engine, counted) = engine(dir.path());
        counted.bulk.store(40_000, Ordering::SeqCst);
        counted.slow.store(true, Ordering::SeqCst);
        engine.with_cache(kept.clone(), limit)
    };
    cached(u64::MAX).check(&[], &[]).unwrap();
    let limit = tree(&kept) * 9 / 10;

    // Each step edits one file, or puts one back: only that file's findings
    // are made again, however full the directory is. What a trim takes is what
    // is big and quick to make, never the findings that cost a rule to make.
    let steps = [(3, 1), (3, 0), (11, 1), (11, 0), (3, 2), (20, 1), (3, 0)];
    for (file, version) in steps {
        fs::write(dir.path().join(format!("f{file}.txt")), text(file, version)).unwrap();
        let run = cached(limit).check(&[], &[]).unwrap();
        let (plain, _) = engine(dir.path());
        assert_eq!(found(&run), found(&plain.check(&[], &[]).unwrap()));
        let ran = run.timings.findings_run;
        assert!(ran <= 1, "file {file} v{version}: {ran} findings run");
        let again = cached(limit).check(&[], &[]).unwrap();
        assert_eq!(
            again.timings.findings_run, 0,
            "file {file} v{version}: warm"
        );
    }
}
