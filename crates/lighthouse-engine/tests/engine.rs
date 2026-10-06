use std::{fs, path::PathBuf};

use lighthouse_config::{Config, RuleConfig, Rules};
use lighthouse_engine::{Engine, Error};
use lighthouse_model::{
    Capability, Diagnostic, File, Fingerprint, Fragment, Options, Position, Severity, Span,
};
use lighthouse_plugin::{
    Analyzer, Conventions, Ctx, Error as PluginError, LanguageProvider, Manifest, Plugin, Preset,
    Registry, Rule, RuleMeta, Scope, Workspace,
};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Any {
    globs: Vec<String>,
    fail_on: &'static str,
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
    fn index(&self, _: &Workspace, file: &File, _: &str) -> Result<Fragment, PluginError> {
        if file.path.to_string_lossy().ends_with(self.fail_on) {
            return Err(PluginError::Failed("cannot index".to_owned()));
        }
        Ok(Fragment {
            files: vec![file.clone()],
            ..Fragment::default()
        })
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
    let mut registry = Registry::default();
    registry.register(&FakePlugin { fail_on }).unwrap();
    registry
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
fn file_scope_reports_are_relative_sorted_and_subset_filtered() {
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
fn invalid_utf8_and_index_failures_become_notices() {
    let dir = project(&[
        ("ok.txt", b"1"),
        ("bin.dat", &[0xff, 0xfe]),
        ("bad.skip", b"x"),
    ]);
    let engine = Engine::new(registry(".skip"), Config::parse(ALL).unwrap(), dir.path()).unwrap();
    let out = engine
        .check(&root(&dir), &["fake/each".to_owned()])
        .unwrap();
    assert!(out.notices.contains("bin.dat: skipped, not valid UTF-8"));
    assert!(out.notices.contains("bad.skip: skipped, cannot index"));
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.file.to_str() == Some("ok.txt"))
    );
}

#[test]
fn paths_outside_the_root_are_skipped_with_notice() {
    let dir = project(&[("x.txt", b"1")]);
    let outside = project(&[("y.txt", b"1")]);
    let out = engine(&dir, ALL)
        .unwrap()
        .check(&[outside.path().to_owned()], &["fake/each".to_owned()])
        .unwrap();
    assert!(out.diagnostics.is_empty());
    assert_eq!(out.notices.len(), 1);
    assert!(out.notices.iter().next().unwrap().contains("outside"));
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
fn exit_code_follows_severity_and_strict() {
    let dir = project(&[("x.txt", b"1")]);
    let code = |level: &str, strict| {
        let toml = format!("plugins = [\"fake\"]\n[rules]\n\"fake/each\" = \"{level}\"\n");
        engine(&dir, &toml)
            .unwrap()
            .check(&root(&dir), &[])
            .unwrap()
            .exit_code(strict)
    };
    assert_eq!(code("error", false), 1);
    assert_eq!(code("warn", false), 0);
    assert_eq!(code("warn", true), 1);
    assert_eq!(code("review", true), 0);
    assert_eq!(code("info", true), 0);
    assert_eq!(code("off", true), 0);
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
