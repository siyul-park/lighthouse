//! The engine's result cache over rules that count their runs: what a run
//! takes from the cache, what makes it run again, and what happens when the
//! cache cannot be used.

use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use lighthouse_engine::{Engine, Outcome};
use lighthouse_model::{
    Applicability, Diagnostic, Fingerprint, Fragment, Options, Position, Reach, RunScope, Severity,
    Span,
};
use lighthouse_plugin::{
    Caching, Ctx, Error as PluginError, Indexed, LanguageProvider, Plugin, PluginManifest,
    ProviderManifest, Registry, Rule, RuleManifest, Source, Workspace,
};
use lighthouse_spec::{Catalog, Config};
use tempfile::TempDir;

const CONFIG: &str = "plugins = [\"counting\"]\n[rules]\n\"counting/local\" = \"warn\"\n\"counting/global\" = \"warn\"\n";

struct Files(ProviderManifest);

impl LanguageProvider for Files {
    fn manifest(&self) -> &ProviderManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, PluginError> {
        Ok(Indexed {
            fragments: files
                .iter()
                .map(|source| Fragment {
                    files: vec![source.file.clone()],
                    ..Fragment::default()
                })
                .collect(),
            ..Indexed::default()
        })
    }
}

/// A rule that counts the times it runs and says what it saw.
struct Counting {
    meta: RuleManifest,
    reach: Reach,
    runs: Arc<AtomicUsize>,
}

impl Rule for Counting {
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }
    fn validate(&self, _: &Options) -> Result<(), PluginError> {
        Ok(())
    }
    fn check(&self, ctx: &Ctx, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        ctx.notices.push(format!("{} ran", self.meta.id));
        let (file, message) = match ctx.file {
            Some((file, text)) => (file.path.clone(), format!("{} bytes", text.len())),
            None => (
                ctx.project.files[0].path.clone(),
                format!("{} files", ctx.project.files.len()),
            ),
        };
        let at = Position { line: 1, col: 1 };
        let fingerprint = Fingerprint::of(&self.meta.id, &file.to_string_lossy(), "");
        Ok(vec![Diagnostic::new(
            &self.meta.id,
            Severity::Warn,
            message,
            file,
            Span { start: at, end: at },
            fingerprint,
        )])
    }
    fn caching(&self) -> Option<Caching> {
        Some(Caching {
            reach: self.reach,
            revision: "1".to_owned(),
        })
    }
}

/// The rules of the plugin and the counters of their runs.
struct Counted {
    local: Arc<AtomicUsize>,
    global: Arc<AtomicUsize>,
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
        vec![Box::new(Files(ProviderManifest::new(
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
        };
        vec![
            Box::new(Counting {
                meta: meta("counting/local", RunScope::File),
                reach: Reach::Local,
                runs: Arc::clone(&self.counted.local),
            }),
            Box::new(Counting {
                meta: meta("counting/global", RunScope::Project),
                reach: Reach::Global,
                runs: Arc::clone(&self.counted.global),
            }),
        ]
    }
}

fn engine(root: &Path) -> (Engine, Counted) {
    let counted = Counted {
        local: Arc::default(),
        global: Arc::default(),
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
            },
        })
        .unwrap();
    let engine = Engine::new(
        registry,
        Config::parse_inline(CONFIG).unwrap(),
        Catalog::bundled(),
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
fn found(outcome: &Outcome) -> Vec<(String, String)> {
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
fn engine_with_cache_takes_findings_instead_of_running_rules_again() {
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
        (second.timings.cache_hits, second.timings.cache_misses),
        (3, 0)
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
}

#[test]
fn engine_with_cache_keeps_the_size_under_its_limit() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let (engine, counted) = engine(dir.path());
        let engine = engine.with_cache(cache.path().join("cache"), 1);
        engine.check(&[], &[]).unwrap();
        assert_eq!(runs(&counted), (2, 1), "everything is pruned at the end");
    }
}

#[test]
fn engine_with_cache_runs_without_it_when_it_cannot_be_opened() {
    let dir = project();
    let cache = tempfile::tempdir().unwrap();
    let blocked = cache.path().join("cache");
    fs::write(&blocked, "a file where the directory belongs").unwrap();
    let (engine, counted) = engine(dir.path());
    let outcome = engine
        .with_cache(blocked, u64::MAX)
        .check(&[], &[])
        .unwrap();
    assert_eq!(runs(&counted), (2, 1));
    assert!(
        outcome
            .notices
            .iter()
            .any(|n| n.starts_with("the result cache is not used")),
        "{:?}",
        outcome.notices
    );
}
