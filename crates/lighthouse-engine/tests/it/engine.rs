use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use lighthouse_engine::{Engine, Error};
use lighthouse_model::{Applicability, RunScope};
use lighthouse_model::{
    Capability, Diagnostic, Fingerprint, Fragment, Incomplete, Options, Position, Severity, Span,
};
use lighthouse_plugin::{
    Analyzer, AnalyzerManifest, Conventions, Ctx, Error as PluginError, Indexed, LanguageProvider,
    Plugin, PluginManifest, ProviderManifest, Registry, Rule, RuleManifest, Source, Workspace,
};
use lighthouse_spec::Catalog;
use lighthouse_spec::Config;
use serde_json::{Value, json};
use tempfile::TempDir;

/// Records the size of every batch it is asked to index.
type Batches = Arc<Mutex<Vec<usize>>>;

struct Any {
    manifest: ProviderManifest,
    /// Files with this suffix are reported incomplete.
    fail_on: &'static str,
    /// A batch containing a file with this suffix fails as a whole.
    crash_on: &'static str,
    batches: Batches,
}

impl LanguageProvider for Any {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }
    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, PluginError> {
        self.batches.lock().unwrap().push(files.len());
        let ends = |s: &Source, suffix: &str| s.file.path.to_string_lossy().ends_with(suffix);
        if files.iter().any(|s| ends(s, self.crash_on)) {
            return Err(PluginError::Failed("plugin crashed".to_owned()));
        }
        let mut indexed = Indexed::default();
        for source in files {
            if ends(source, self.fail_on) {
                indexed.incomplete.push(Incomplete {
                    path: Some(source.file.path.clone()),
                    reason: "cannot index".to_owned(),
                });
                continue;
            }
            indexed.fragments.push(Fragment {
                files: vec![source.file.clone()],
                ..Fragment::default()
            });
        }
        Ok(indexed)
    }
}

/// Counts files in the merged project; project scope.
struct CountFiles(AnalyzerManifest);

impl Analyzer for CountFiles {
    fn manifest(&self) -> &AnalyzerManifest {
        &self.0
    }
    fn run(&self, ctx: &Ctx) -> Result<Value, PluginError> {
        Ok(json!(ctx.project.files.len()))
    }
}

struct Fake {
    meta: RuleManifest,
}

fn meta(
    id: &str,
    scope: RunScope,
    analyzers: &[&str],
    capabilities: &[Capability],
) -> RuleManifest {
    RuleManifest {
        id: id.to_owned(),
        severity: Severity::Warn,
        scope,
        description: String::new(),
        docs: String::new(),
        analyzers: analyzers.iter().map(|a| (*a).to_owned()).collect(),
        capabilities: capabilities.to_vec(),
        applicability: Applicability::default(),
    }
}

impl Rule for Fake {
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }
    fn validate(&self, options: &Options) -> Result<(), PluginError> {
        match options.keys().next() {
            Some(k) if k != "ok" => Err(PluginError::Options {
                rule: self.meta.id.clone(),
                message: k.clone(),
            }),
            _ => Ok(()),
        }
    }
    fn check(&self, ctx: &Ctx, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let at = Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 1 },
        };
        let (file, message) = match ctx.file {
            Some((file, _)) if ctx.trusted => (file.path.clone(), "file (trusted)".to_owned()),
            Some((file, _)) => (file.path.clone(), "file".to_owned()),
            None => {
                let count: usize = ctx.fact("fake/count-files")?;
                (ctx.project.files[0].path.clone(), format!("{count} files"))
            }
        };
        let fp = Fingerprint::of(&self.meta.id, &file.to_string_lossy(), "");
        let d = Diagnostic::new(&self.meta.id, Severity::Info, message, file, at, fp);
        Ok(vec![d.clone(), d])
    }
}

struct FakePlugin {
    manifest: PluginManifest,
    fail_on: &'static str,
    crash_on: &'static str,
    batches: Batches,
}

impl Plugin for FakePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Any {
            manifest: ProviderManifest {
                conventions: Conventions {
                    test_globs: vec!["tests/**".to_owned()],
                },
                ..ProviderManifest::new("any", vec!["**".to_owned()])
            },
            fail_on: self.fail_on,
            crash_on: self.crash_on,
            batches: Arc::clone(&self.batches),
        })]
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        vec![Box::new(CountFiles(AnalyzerManifest {
            id: "fake/count-files".to_owned(),
            requires: Vec::new(),
            scope: RunScope::Project,
        }))]
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![
            Box::new(Fake {
                meta: meta("fake/each", RunScope::File, &[], &[]),
            }),
            Box::new(Fake {
                meta: meta("fake/all", RunScope::Project, &["fake/count-files"], &[]),
            }),
            Box::new(Fake {
                meta: meta(
                    "fake/semantic",
                    RunScope::File,
                    &[],
                    &[Capability::SemanticEdges],
                ),
            }),
        ]
    }
}

fn registry(fail_on: &'static str) -> Registry {
    batched(fail_on, "!").0
}

fn batched(fail_on: &'static str, crash_on: &'static str) -> (Registry, Batches) {
    let batches = Batches::default();
    let mut registry = Registry::default();
    registry
        .register(&FakePlugin {
            manifest: plugin_manifest("fake"),
            fail_on,
            crash_on,
            batches: Arc::clone(&batches),
        })
        .unwrap();
    (registry, batches)
}

fn plugin_manifest(id: &str) -> PluginManifest {
    PluginManifest {
        id: id.to_owned(),
        version: "0".to_owned(),
    }
}

/// A fallback provider, as a plain-text plugin would be.
struct Fallback(ProviderManifest);

impl Fallback {
    fn new() -> Self {
        Self(ProviderManifest {
            fallback: true,
            ..ProviderManifest::new("bin", vec!["**".to_owned()])
        })
    }
}

impl LanguageProvider for Fallback {
    fn manifest(&self) -> &ProviderManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, PluginError> {
        Ok(Indexed {
            fragments: files
                .iter()
                .map(|s| Fragment {
                    files: vec![s.file.clone()],
                    ..Fragment::default()
                })
                .collect(),
            ..Indexed::default()
        })
    }
}

struct FallbackPlugin(PluginManifest);

impl Plugin for FallbackPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Fallback::new())]
    }
}

fn project(files: &[(&str, &[u8])]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in files {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    dir
}

fn engine(dir: &TempDir, toml: &str) -> Result<Engine, Error> {
    Engine::new(
        registry("!"),
        Config::parse_inline(toml).unwrap(),
        Catalog::bundled(),
        dir.path(),
    )
}

const ALL: &str = r#"plugins = ["fake"]
[rules]
"fake/each" = "error"
"fake/all" = "warn"
"fake/semantic" = "warn"
"#;

fn root(dir: &TempDir) -> Vec<PathBuf> {
    vec![dir.path().to_owned()]
}

#[test]
fn project_scope_analyzer_sees_every_file_even_when_reporting_a_subset() {
    let dir = project(&[("a/x.txt", b"1"), ("b/y.txt", b"2"), ("b/z.txt", b"3")]);
    let engine = engine(&dir, ALL).unwrap();
    let out = engine
        .check(&[dir.path().join("b")], &["fake/all".to_owned()])
        .unwrap();
    // a/x.txt sorts first, so the project diagnostic lands outside `b` and is filtered.
    assert!(out.diagnostics.is_empty());
    let out = engine
        .check(&[dir.path().join("a")], &["fake/all".to_owned()])
        .unwrap();
    assert!(
        out.diagnostics.iter().all(|d| d.message == "3 files"),
        "{:?}",
        out.diagnostics
    );
    assert_eq!(out.diagnostics.len(), 2);
}

#[test]
fn outcome() {
    let dir = project(&[("a/x.txt", b"1"), ("b/y.txt", b"2")]);
    let engine = engine(
        &dir,
        "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"error\"\n",
    )
    .unwrap();
    let out = engine.check(&[dir.path().join("b")], &[]).unwrap();
    let files: Vec<_> = out
        .diagnostics
        .iter()
        .map(|d| d.file.to_str().unwrap())
        .collect();
    assert_eq!(files, ["b/y.txt", "b/y.txt"]);
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.severity == Severity::Error)
    );
}

#[test]
fn identical_findings_get_distinct_fingerprints() {
    let dir = project(&[("x.txt", b"1")]);
    let engine = engine(
        &dir,
        "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"warn\"\n",
    )
    .unwrap();
    let out = engine.check(&root(&dir), &[]).unwrap();
    assert_eq!(out.diagnostics.len(), 2);
    assert_ne!(
        out.diagnostics[0].fingerprint,
        out.diagnostics[1].fingerprint
    );
}

#[test]
fn missing_capability_skips_with_notice() {
    let dir = project(&[("x.txt", b"1")]);
    let out = engine(&dir, ALL)
        .unwrap()
        .check(&root(&dir), &["fake/semantic".to_owned()])
        .unwrap();
    assert!(out.diagnostics.is_empty());
    assert!(
        out.notices
            .iter()
            .any(|n| n.contains("fake/semantic") && n.contains("semantic-edges"))
    );
}

#[test]
fn unreadable_and_unindexed_files_are_incomplete_not_notices() {
    let dir = project(&[
        ("ok.txt", b"1"),
        ("bin.dat", &[0xff, 0xfe]),
        ("bad.skip", b"x"),
    ]);
    let engine = Engine::new(
        registry(".skip"),
        Config::parse_inline(ALL).unwrap(),
        Catalog::bundled(),
        dir.path(),
    )
    .unwrap();
    let out = engine
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();
    let gaps: Vec<_> = out
        .incomplete
        .iter()
        .map(|i| {
            (
                i.path.as_deref().and_then(|p| p.to_str()),
                i.reason.as_str(),
            )
        })
        .collect();
    assert_eq!(
        gaps,
        [
            (Some("bad.skip"), "cannot index"),
            (Some("bin.dat"), "not valid UTF-8"),
        ]
    );
    assert!(out.notices.is_empty(), "{:?}", out.notices);
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.file.to_str() == Some("ok.txt"))
    );
}

#[test]
fn non_utf8_data_claimed_only_by_a_fallback_provider_is_a_notice() {
    let dir = project(&[("ok.txt", b"1"), ("image.png", &[0xff, 0xfe])]);
    let mut registry = Registry::default();
    registry
        .register(&FallbackPlugin(plugin_manifest("bin")))
        .unwrap();
    let engine = Engine::new(
        registry,
        Config::parse_inline("plugins = [\"bin\"]").unwrap(),
        Catalog::bundled(),
        dir.path(),
    )
    .unwrap();
    let out = engine.check(&root(&dir), &[]).unwrap();
    assert!(out.incomplete.is_empty());
    assert!(out.notices.contains("image.png: skipped, not valid UTF-8"));
}

#[test]
fn a_provider_with_an_execution_error_makes_its_whole_batch_incomplete_in_one_entry() {
    let dir = project(&[("a.txt", b"1"), ("b.txt", b"2"), ("boom.crash", b"3")]);
    let (registry, batches) = batched("!", ".crash");
    let engine = Engine::new(
        registry,
        Config::parse_inline(ALL).unwrap(),
        Catalog::bundled(),
        dir.path(),
    )
    .unwrap();
    let out = engine
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();
    assert!(out.diagnostics.is_empty());
    assert_eq!(out.incomplete.len(), 1);
    let gap = &out.incomplete[0];
    assert_eq!(gap.path, None);
    assert!(
        gap.reason.contains("language `any` had an execution error"),
        "{}",
        gap.reason
    );
    assert!(gap.reason.contains("3 file(s)") && gap.reason.contains("plugin crashed"));
    assert_eq!(*batches.lock().unwrap(), [3]);
}

#[test]
fn each_provider_is_called_once_with_all_its_files() {
    let dir = project(&[("a/x.txt", b"1"), ("b/y.txt", b"2"), ("z.txt", b"3")]);
    let (registry, batches) = batched("!", "!");
    let engine = Engine::new(
        registry,
        Config::parse_inline(ALL).unwrap(),
        Catalog::bundled(),
        dir.path(),
    )
    .unwrap();
    engine.check(&root(&dir), &[]).unwrap();
    engine.check(&[dir.path().join("a")], &[]).unwrap();
    assert_eq!(*batches.lock().unwrap(), [3, 3]);
}

#[test]
fn paths_outside_the_root_are_incomplete() {
    let dir = project(&[("x.txt", b"1")]);
    let outside = project(&[("y.txt", b"1")]);
    let out = engine(&dir, ALL)
        .unwrap()
        .check(&[outside.path().to_owned()], &["fake/each".to_owned()])
        .unwrap();
    assert!(out.diagnostics.is_empty());
    assert!(out.notices.is_empty());
    assert_eq!(out.incomplete.len(), 1);
    assert!(out.incomplete[0].reason.contains("outside"));
    assert_eq!(out.incomplete[0].path.as_deref(), Some(outside.path()));
}

#[test]
fn only_must_name_an_enabled_rule() {
    let dir = project(&[("x.txt", b"1")]);
    let engine = engine(
        &dir,
        "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"warn\"\n",
    )
    .unwrap();
    assert!(matches!(
        engine.check(&root(&dir), &["fake/all".to_owned()]),
        Err(Error::RuleNotEnabled(_))
    ));
    assert!(matches!(
        engine.check(&root(&dir), &["fake/nope".to_owned()]),
        Err(Error::UnknownRule(_))
    ));
}

#[test]
fn outcome_exit_code() {
    let dir = project(&[("x.txt", b"1"), ("bad.skip", b"x")]);
    let code = |level: &str, fail_on: lighthouse_engine::FailOn| {
        let toml = format!("plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"{level}\"\n");
        engine(&dir, &toml)
            .unwrap()
            .check(&root(&dir), &[])
            .unwrap()
            .exit_code(fail_on, false)
    };
    let lenient = lighthouse_engine::FailOn::default();
    let strict = lighthouse_engine::FailOn::from(true);
    let at_most = |n| lighthouse_engine::FailOn {
        strict: false,
        max_warnings: Some(n),
    };
    assert_eq!(code("error", lenient), 1);
    assert_eq!(code("warn", lenient), 0);
    assert_eq!(code("warn", strict), 1);
    assert_eq!(code("info", strict), 0);
    assert_eq!(code("off", strict), 0);
    // Warnings are tolerated up to the number given, and fail past it.
    let warnings = engine(
        &dir,
        "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"warn\"\n",
    )
    .unwrap()
    .check(&root(&dir), &[])
    .unwrap()
    .diagnostics
    .len();
    assert!(warnings > 0);
    assert_eq!(code("warn", at_most(warnings)), 0);
    assert_eq!(code("warn", at_most(warnings - 1)), 1);
    assert_eq!(code("info", at_most(0)), 0, "info never fails a run");
    assert_eq!(code("error", at_most(5)), 1, "errors always do");

    let engine = Engine::new(
        registry(".skip"),
        Config::parse_inline(ALL).unwrap(),
        Catalog::bundled(),
        dir.path(),
    )
    .unwrap();
    let out = engine
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();
    assert_eq!(out.diagnostics[0].severity, Severity::Error);
    assert_eq!(
        out.exit_code(false, false),
        lighthouse_engine::EXIT_INCOMPLETE
    );
    assert_eq!(out.exit_code(true, false), 3);
    assert_eq!(out.exit_code(false, true), 1);
}

#[test]
fn construction_validates_plugins_presets_rules_and_options() {
    let dir = project(&[("x.txt", b"1")]);
    let err = |toml: &str| engine(&dir, toml).err().expect("config is invalid");
    assert!(matches!(
        err("plugins = [\"nope\"]"),
        Error::UnknownPlugin(_)
    ));
    assert!(matches!(
        err("plugins = [\"fake\"]\nextends = [\"fake/zzz\"]"),
        Error::Project(_)
    ));
    assert!(matches!(
        err("plugins = [\"fake\"]\n[rules]\n\"fake/zzz\" = \"warn\""),
        Error::UnknownRule(_)
    ));
    assert!(matches!(
        err("[rules]\n\"fake/each\" = \"warn\""),
        Error::PluginNotListed(_)
    ));
    assert!(matches!(
        err(
            "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = { level = \"warn\", options = { bad = 1 } }"
        ),
        Error::Plugin(PluginError::Options { .. })
    ));
    assert!(matches!(
        err(
            "plugins = [\"fake\"]\n[[overrides]]\nrules = { \"fake/each\" = { level = \"warn\", options = { bad = 1 } } }"
        ),
        Error::Plugin(PluginError::Options { .. })
    ));
}

#[test]
fn languages_come_only_from_listed_plugins() {
    let dir = project(&[("x.txt", b"1")]);
    let out = engine(&dir, "[rules]\n")
        .unwrap()
        .check(&root(&dir), &[])
        .unwrap();
    assert!(out.diagnostics.is_empty());
    assert!(out.notices.is_empty());
}

#[test]
fn engine_check_files() {
    let dir = project(&[("a/x.txt", b"1"), ("b/y.txt", b"2")]);
    let engine = engine(
        &dir,
        "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"error\"\n",
    )
    .unwrap();
    let out = engine
        .check_files(&[PathBuf::from("b/y.txt")], &[])
        .unwrap();
    assert!(!out.diagnostics.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.file == Path::new("b/y.txt"))
    );
    assert!(engine.check_files(&[], &[]).unwrap().diagnostics.is_empty());
}

#[test]
fn engine_with_trust_tells_the_rules_the_user_trusts_the_project() {
    let dir = project(&[("x.txt", b"1")]);
    let untrusted = engine(&dir, ALL).unwrap();
    let before = untrusted
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();
    assert!(before.diagnostics.iter().all(|d| d.message == "file"));

    let trusted = engine(&dir, ALL).unwrap().with_trust(true);
    let after = trusted
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();

    assert!(!after.diagnostics.is_empty());
    assert!(
        after
            .diagnostics
            .iter()
            .all(|d| d.message == "file (trusted)")
    );
}

#[test]
fn engine_with_incomplete() {
    let dir = project(&[("x.txt", b"1")]);
    let gap = Incomplete {
        path: None,
        reason: "plugin failed to start".to_owned(),
    };
    let engine = engine(&dir, ALL).unwrap();
    let engine = engine.with_incomplete(vec![gap.clone()]);
    for _ in 0..2 {
        let out = engine.check(&root(&dir), &[]).unwrap();
        assert_eq!(out.incomplete, std::slice::from_ref(&gap));
    }
}

#[test]
fn engine_check_gives_the_same_outcome_on_every_run_however_the_rules_interleave() {
    let names: Vec<String> = (0..48).map(|i| format!("d{}/f{i}.txt", i % 7)).collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    let dir = project(&files);
    let engine = engine(&dir, ALL).unwrap();
    let first = engine.check(&[], &[]).unwrap();
    assert!(first.diagnostics.len() > names.len());

    for _ in 0..8 {
        let again = engine.check(&[], &[]).unwrap();

        assert_eq!(again.diagnostics, first.diagnostics);
        assert_eq!(again.facts, first.facts);
        assert_eq!(again.options, first.options);
        assert_eq!(again.notices, first.notices);
        assert_eq!(again.incomplete, first.incomplete);
    }
}

#[test]
fn timings() {
    let dir = project(&[("a/x.txt", b"1"), ("b/y.txt", b"2")]);
    let engine = engine(&dir, ALL).unwrap();

    let timings = engine.check(&[], &[]).unwrap().timings;

    let indexed: Vec<&str> = timings.index.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(indexed, ["any"]);
    let mut ruled: Vec<&str> = timings.rules.iter().map(|(id, _)| id.as_str()).collect();
    ruled.sort_unstable();
    assert_eq!(ruled, ["fake/all", "fake/each"]);
    assert!(timings.rules.windows(2).all(|pair| pair[0].1 >= pair[1].1));
}

#[test]
fn outcome_states_report_scope_rules_that_ran_and_subject_facts() {
    let dir = project(&[("a/x.txt", b"1"), ("b/y.txt", b"2")]);
    let engine = engine(&dir, ALL).unwrap();

    let by_path = engine
        .check(&[dir.path().join("b")], &["fake/each".to_owned()])
        .unwrap();
    assert_eq!(by_path.reported, [PathBuf::from("b")]);
    assert_eq!(by_path.rules, ["fake/each"]);
    let facts = &by_path.facts[&by_path.diagnostics[0].fingerprint];
    assert_eq!(facts["language"], "any");

    let whole = engine.check(&[], &[]).unwrap();
    assert_eq!(whole.reported, [PathBuf::new()]);
    assert_eq!(whole.rules, ["fake/all", "fake/each", "fake/semantic"]);

    let none = engine.check_files(&[], &[]).unwrap();
    assert!(none.reported.is_empty());
    let files = engine.check_files(&["a/x.txt".into()], &[]).unwrap();
    assert_eq!(files.reported, [PathBuf::from("a/x.txt")]);
}

const TEAM: &str = "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: fake/p\nspec:\n  rules:\n    fake/each: warn\n";

#[test]
fn active_rules() {
    let config = |toml: &str| Config::parse_inline(toml).unwrap();
    let team =
        Catalog::from_local(BTreeMap::from([("team.yaml".to_owned(), TEAM.to_owned())])).unwrap();
    let projects = Catalog::overlay(Catalog::bundled(), &team)
        .unwrap()
        .projects()
        .unwrap();
    let from_project = lighthouse_engine::active_rules(
        &config("plugins = [\"fake\"]\nextends = [\"fake/p\"]\n"),
        &projects,
    )
    .unwrap();
    assert_eq!(from_project.into_iter().collect::<Vec<_>>(), ["fake/each"]);

    let set_by_entry = lighthouse_engine::active_rules(
        &config("plugins = [\"fake\"]\n[rules]\n\"fake/all\" = \"warn\"\n"),
        &projects,
    )
    .unwrap();
    assert_eq!(set_by_entry.into_iter().collect::<Vec<_>>(), ["fake/all"]);

    let unknown = lighthouse_engine::active_rules(&config("extends = [\"nope/p\"]\n"), &projects);
    assert!(unknown.is_err());
}

#[test]
fn hash_of_is_the_sha256_hex_the_model_records() {
    assert_eq!(
        lighthouse_model::hash::sha256("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn an_overlaid_check_reads_the_overlay_not_the_disk_and_leaves_the_disk_alone() {
    let dir = project(&[("a.txt", b"on disk"), ("b.txt", b"other")]);
    let engine = engine(&dir, ALL).unwrap();
    let overlays = lighthouse_engine::Overlays::from([("a.txt".into(), "overlaid".to_owned())]);

    let out = engine.check_overlaid(&[], &[], &overlays).unwrap();
    let plain = engine.check(&[], &[]).unwrap();

    let hash = |o: &lighthouse_engine::Outcome, f: &str| {
        o.project.file(Path::new(f)).unwrap().hash.clone()
    };
    assert_eq!(
        hash(&out, "a.txt"),
        lighthouse_model::hash::sha256("overlaid")
    );
    assert_eq!(
        hash(&plain, "a.txt"),
        lighthouse_model::hash::sha256("on disk")
    );
    assert_eq!(hash(&out, "b.txt"), hash(&plain, "b.txt"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "on disk"
    );
}

/// The files `fake/each` reported on.
fn reported(dir: &TempDir, toml: &str) -> Vec<String> {
    let out = engine(dir, toml)
        .unwrap()
        .check(&[], &["fake/each".to_owned()])
        .unwrap();
    let mut files: Vec<String> = out
        .diagnostics
        .iter()
        .map(|d| d.file.to_string_lossy().into_owned())
        .collect();
    files.dedup();
    files
}

#[test]
fn test_code_is_not_a_subject_unless_the_scope_says_so() {
    let dir = project(&[("a/x.txt", b"1"), ("tests/t.txt", b"2")]);

    assert_eq!(reported(&dir, ALL), ["a/x.txt"]);
}

#[test]
fn generated_code_is_told_apart_by_the_host_and_left_out_of_the_subjects() {
    let dir = project(&[
        ("a/x.txt", b"1"),
        ("gen/g.txt", b"2"),
        ("vendor/v.txt", b"3"),
        (
            ".gitattributes",
            b"gen/** linguist-generated=true\n*.md -linguist-generated\n",
        ),
    ]);

    assert_eq!(reported(&dir, ALL), ["a/x.txt", "vendor/v.txt"]);
    let by_config = format!("{ALL}[generated]\nfiles = [\"vendor/**\"]\n");
    assert_eq!(reported(&dir, &by_config), ["a/x.txt"]);
}

#[test]
fn the_project_decides_whether_generated_code_is_checked() {
    let dir = project(&[
        ("a/x.txt", b"1"),
        ("gen/g.txt", b"2"),
        (".gitattributes", b"gen/** linguist-generated\n"),
    ]);
    let include = "plugins = [\"fake\"]\n[generated]\ncheck = \"include\"\n[rules]\n\"fake/each\" = \"error\"\n";
    let one_rule =
        "plugins = [\"fake\"]\n[rules]\n\"fake/each\" = { level = \"error\", generated = true }\n";
    let skip_over_rule = "plugins = [\"fake\"]\n[generated]\ncheck = \"skip\"\n[rules]\n\"fake/each\" = { level = \"error\", generated = true }\n";
    let all = ["a/x.txt", "gen/g.txt"];

    assert_eq!(reported(&dir, include), all);
    assert_eq!(reported(&dir, one_rule), all);
    assert_eq!(
        reported(&dir, skip_over_rule),
        all,
        "a rule's own setting wins over the project's"
    );
}
