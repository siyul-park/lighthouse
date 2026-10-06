use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use lighthouse_config::{Config, RuleConfig, Rules};
use lighthouse_engine::{Engine, Error};
use lighthouse_model::{
    Capability, Diagnostic, Fingerprint, Fragment, Incomplete, Options, Position, Severity, Span,
};
use lighthouse_plugin::{
    Analyzer, Conventions, Ctx, Error as PluginError, Indexed, LanguageProvider, Manifest, Plugin,
    Preset, Registry, Rule, RuleMeta, Scope, Source, Workspace,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Records the size of every batch it is asked to index.
type Batches = Arc<Mutex<Vec<usize>>>;

struct Any {
    globs: Vec<String>,
    /// Files with this suffix are reported incomplete.
    fail_on: &'static str,
    /// A batch containing a file with this suffix fails as a whole.
    crash_on: &'static str,
    batches: Batches,
}

impl LanguageProvider for Any {
    fn id(&self) -> &str {
        "any"
    }
    fn globs(&self) -> &[String] {
        &self.globs
    }
    fn conventions(&self) -> Conventions {
        Conventions {
            test_globs: vec!["tests/**".to_owned()],
        }
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
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
struct CountFiles;

impl Analyzer for CountFiles {
    fn id(&self) -> &str {
        "fake/count-files"
    }
    fn requires(&self) -> &[String] {
        &[]
    }
    fn scope(&self) -> Scope {
        Scope::Project
    }
    fn run(&self, ctx: &Ctx) -> Result<Value, PluginError> {
        Ok(json!(ctx.project.files.len()))
    }
}

struct Fake {
    meta: RuleMeta,
}

fn meta(id: &str, scope: Scope, analyzers: &[&str], capabilities: &[Capability]) -> RuleMeta {
    RuleMeta {
        id: id.to_owned(),
        severity: Severity::Warn,
        scope,
        description: String::new(),
        docs: String::new(),
        analyzers: analyzers.iter().map(|a| (*a).to_owned()).collect(),
        capabilities: capabilities.to_vec(),
        citation: None,
        strict: false,
    }
}

impl Rule for Fake {
    fn meta(&self) -> &RuleMeta {
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
    fail_on: &'static str,
    crash_on: &'static str,
    batches: Batches,
}

impl Plugin for FakePlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "fake".to_owned(),
            version: "0".to_owned(),
        }
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Any {
            globs: vec!["**".to_owned()],
            fail_on: self.fail_on,
            crash_on: self.crash_on,
            batches: Arc::clone(&self.batches),
        })]
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        vec![Box::new(CountFiles)]
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![
            Box::new(Fake {
                meta: meta("fake/each", Scope::File, &[], &[]),
            }),
            Box::new(Fake {
                meta: meta("fake/all", Scope::Project, &["fake/count-files"], &[]),
            }),
            Box::new(Fake {
                meta: meta(
                    "fake/semantic",
                    Scope::File,
                    &[],
                    &[Capability::SemanticEdges],
                ),
            }),
        ]
    }
    fn presets(&self) -> Vec<Preset> {
        let each = RuleConfig {
            level: Some(Severity::Warn),
            options: Options::new(),
        };
        vec![Preset {
            id: "fake/p".to_owned(),
            rules: Rules::from([("fake/each".to_owned(), each)]),
        }]
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
            fail_on,
            crash_on,
            batches: Arc::clone(&batches),
        })
        .unwrap();
    (registry, batches)
}

/// A fallback provider, as a plain-text plugin would be.
struct Fallback;

impl LanguageProvider for Fallback {
    fn id(&self) -> &str {
        "bin"
    }
    fn globs(&self) -> &[String] {
        static ALL: std::sync::LazyLock<Vec<String>> =
            std::sync::LazyLock::new(|| vec!["**".to_owned()]);
        &ALL
    }
    fn conventions(&self) -> Conventions {
        Conventions::default()
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
    fn fallback(&self) -> bool {
        true
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

struct FallbackPlugin;

impl Plugin for FallbackPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "bin".to_owned(),
            version: "0".to_owned(),
        }
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Fallback)]
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
    Engine::new(registry("!"), Config::parse(toml).unwrap(), dir.path())
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
    let engine = Engine::new(registry(".skip"), Config::parse(ALL).unwrap(), dir.path()).unwrap();
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
    registry.register(&FallbackPlugin).unwrap();
    let engine = Engine::new(
        registry,
        Config::parse("plugins = [\"bin\"]").unwrap(),
        dir.path(),
    )
    .unwrap();
    let out = engine.check(&root(&dir), &[]).unwrap();
    assert!(out.incomplete.is_empty());
    assert!(out.notices.contains("image.png: skipped, not valid UTF-8"));
}

#[test]
fn a_failed_provider_makes_its_whole_batch_incomplete_in_one_entry() {
    let dir = project(&[("a.txt", b"1"), ("b.txt", b"2"), ("boom.crash", b"3")]);
    let (registry, batches) = batched("!", ".crash");
    let engine = Engine::new(registry, Config::parse(ALL).unwrap(), dir.path()).unwrap();
    let out = engine
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();
    assert!(out.diagnostics.is_empty());
    assert_eq!(out.incomplete.len(), 1);
    let gap = &out.incomplete[0];
    assert_eq!(gap.path, None);
    assert!(
        gap.reason.contains("language `any` failed"),
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
    let engine = Engine::new(registry, Config::parse(ALL).unwrap(), dir.path()).unwrap();
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
    let code = |level: &str, strict| {
        let toml = format!("plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"{level}\"\n");
        engine(&dir, &toml)
            .unwrap()
            .check(&root(&dir), &[])
            .unwrap()
            .exit_code(strict, false)
    };
    assert_eq!(code("error", false), 1);
    assert_eq!(code("warn", false), 0);
    assert_eq!(code("warn", true), 1);
    assert_eq!(code("review", true), 0);
    assert_eq!(code("info", true), 0);
    assert_eq!(code("off", true), 0);

    let engine = Engine::new(registry(".skip"), Config::parse(ALL).unwrap(), dir.path()).unwrap();
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
        err("extends = [\"fake/p\"]"),
        Error::PluginNotListed(_)
    ));
    assert!(matches!(
        err("plugins = [\"fake\"]\nextends = [\"fake/zzz\"]"),
        Error::Config(_)
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
        err("plugins = [\"fake\"]\n[rules]\n\"fake/each\" = { level = \"warn\", bad = 1 }"),
        Error::Plugin(PluginError::Options { .. })
    ));
    assert!(matches!(
        err(
            "plugins = [\"fake\"]\n[[overrides]]\nrules = { \"fake/each\" = { level = \"warn\", bad = 1 } }"
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
