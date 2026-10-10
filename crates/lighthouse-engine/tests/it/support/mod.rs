//! A toy language and rules for the fix tests: lines of `fn name` are symbols,
//! `TODO` and `BAD` are findings, and `BROKEN` makes a file unanalyzable.
#![allow(dead_code)]

use lighthouse_spec::Catalog;
use std::{fs, sync::LazyLock};

use lighthouse_engine::{Engine, FixBinding, FixPlan, FixRun};
use lighthouse_model::{Applicability, RunScope};
use lighthouse_model::{
    Capability, Diagnostic, EditOp, File, Fingerprint, FixOutcome, Fragment, Incomplete, LineIndex,
    Options, Position, Safety, Severity, Span, Symbol, SymbolId, SymbolKind, Visibility,
};
use lighthouse_plugin::{
    Ctx, Error as PluginError, FixDecision, FixRequest, Fixer, FixerManifest, Indexed,
    LanguageProvider, Plugin, PluginManifest, ProviderManifest, Registry, Rule, RuleManifest,
    Source, Workspace,
};
use lighthouse_spec::Config;
use tempfile::TempDir;

/// The provider manifest of the toy language: it analyzes overlays.
pub fn toy() -> ProviderManifest {
    ProviderManifest {
        capabilities: vec![lighthouse_model::Capability::Overlays],
        ..ProviderManifest::new("toy", vec!["**/*.toy".to_owned()])
    }
}

pub struct Toy(pub ProviderManifest);

impl LanguageProvider for Toy {
    fn manifest(&self) -> &ProviderManifest {
        &self.0
    }

    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, PluginError> {
        let mut indexed = Indexed::default();
        for source in files {
            let symbols = source
                .text
                .lines()
                .enumerate()
                .filter_map(|(n, line)| {
                    let name = line.strip_prefix("fn ")?.split_whitespace().next()?;
                    let at = |col: usize| Position {
                        line: u32::try_from(n + 1).unwrap(),
                        col: u32::try_from(col + 1).unwrap(),
                    };
                    let span = Span {
                        start: at(0),
                        end: at(line.len()),
                    };
                    Some(Symbol {
                        id: SymbolId::new("m", &[], name, SymbolKind::Function),
                        kind: SymbolKind::Function,
                        visibility: Visibility::Private,
                        owner: None,
                        file: source.file.path.clone(),
                        span,
                        extent: Some(span),
                        doc: None,
                        name: name.to_owned(),
                        role: None,
                    })
                })
                .collect();
            if source.text.contains("BROKEN") {
                indexed.incomplete.push(Incomplete {
                    path: Some(source.file.path.clone()),
                    reason: "cannot parse".to_owned(),
                });
            }
            indexed.fragments.push(Fragment {
                files: vec![File {
                    ..source.file.clone()
                }],
                symbols,
                ..Fragment::default()
            });
        }
        Ok(indexed)
    }
}

/// Flags every occurrence of a word as an error.
pub struct Word {
    manifest: RuleManifest,
    word: &'static str,
}

impl Word {
    pub fn new(id: &str, word: &'static str) -> Self {
        Self {
            manifest: RuleManifest {
                id: id.to_owned(),
                uid: None,
                severity: Severity::Error,
                scope: RunScope::File,
                description: String::new(),
                docs: String::new(),
                analyzers: Vec::new(),
                capabilities: Vec::new(),
                applicability: Applicability::default(),
            },
            word,
        }
    }
}

impl Rule for Word {
    fn manifest(&self) -> &RuleManifest {
        &self.manifest
    }

    fn validate(&self, _: &Options) -> Result<(), PluginError> {
        Ok(())
    }

    fn check(&self, ctx: &Ctx, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let (file, text) = ctx
            .file
            .ok_or_else(|| PluginError::Failed("no file".into()))?;
        let index = LineIndex::new(text);
        Ok(text
            .match_indices(self.word)
            .enumerate()
            .map(|(n, (at, _))| {
                let span = Span {
                    start: index.position(at),
                    end: index.position(at + self.word.len()),
                };
                Diagnostic::new(
                    &self.manifest.id,
                    Severity::Error,
                    format!("{} found", self.word),
                    &file.path,
                    span,
                    Fingerprint::of(&self.manifest.id, &file.path.to_string_lossy(), "")
                        .occurrence(n),
                )
            })
            .collect())
    }
}

/// Replaces the flagged word with `with`.
pub struct Rewrite {
    manifest: FixerManifest,
    with: &'static str,
    safety: Safety,
}

impl Fixer for Rewrite {
    fn manifest(&self) -> &FixerManifest {
        &self.manifest
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        Ok(FixOutcome::Proposed {
            description: format!("replace with {}", self.with),
            ops: vec![EditOp::Replace {
                file: request.finding.file.clone(),
                span: request.finding.span,
                text: self.with.to_owned(),
            }],
            safety: self.safety,
        })
    }
}

/// Rewrites the whole line of the finding, so findings of one line propose
/// the same edit.
pub struct Line(pub FixerManifest);

impl Fixer for Line {
    fn manifest(&self) -> &FixerManifest {
        &self.0
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        let line = request.finding.span.start.line;
        let text = request
            .text
            .lines()
            .nth(line as usize - 1)
            .unwrap_or_default();
        let at = |col: usize| Position {
            line,
            col: u32::try_from(col + 1).unwrap(),
        };
        Ok(FixOutcome::Proposed {
            description: "rewrite the line".to_owned(),
            ops: vec![EditOp::Replace {
                file: request.finding.file.clone(),
                span: Span {
                    start: at(0),
                    end: at(text.len()),
                },
                text: text.replace("TODO", "DONE"),
            }],
            safety: Safety::Safe,
        })
    }
}

pub fn rewrite(id: &str, with: &'static str, safety: Safety, requires: &[Capability]) -> Rewrite {
    Rewrite {
        manifest: FixerManifest {
            id: id.to_owned(),
            requires: requires.to_vec(),
        },
        with,
        safety,
    }
}

pub struct Fake(pub PluginManifest);

impl Plugin for Fake {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Toy(toy()))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![
            Box::new(Word::new("fake/todo", "TODO")),
            Box::new(Word::new("fake/bad", "BAD")),
        ]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![
            Box::new(rewrite("fake/to-done", "DONE", Safety::Safe, &[])),
            Box::new(rewrite("fake/to-bad", "BAD", Safety::Safe, &[])),
            Box::new(rewrite("fake/to-broken", "BROKEN", Safety::Safe, &[])),
            Box::new(rewrite(
                "fake/to-done-claimed-safe",
                "DONE",
                Safety::Safe,
                &[],
            )),
            Box::new(Line(FixerManifest {
                id: "fake/line".to_owned(),
                requires: Vec::new(),
            })),
            Box::new(rewrite(
                "fake/needs-extent",
                "DONE",
                Safety::Safe,
                &[Capability::Extent],
            )),
        ]
    }
}

pub static PLUGIN: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
    id: "fake".to_owned(),
    version: "0".to_owned(),
});

pub fn engine(dir: &TempDir, extra: &str) -> Engine {
    let mut registry = Registry::default();
    registry.register(&Fake(PLUGIN.clone())).unwrap();
    let config = Config::parse_inline(&format!(
        "plugins = [\"fake\"]\n[rules]\n\"fake/todo\" = \"error\"\n\"fake/bad\" = \"error\"\n{extra}"
    ))
    .unwrap();
    Engine::new(registry, config, Catalog::bundled(), dir.path()).unwrap()
}

pub fn project(text: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.toy"), text).unwrap();
    dir
}

pub fn plan(fixer: &str, cap: Safety, mechanical: bool) -> FixPlan {
    let mut plan = FixPlan::default();
    plan.insert(
        "fake/todo",
        FixBinding {
            fixer: fixer.to_owned(),
            cap,
            mechanical,
            decision: FixDecision {
                id: "fake/todo".to_owned(),
                requirement: String::new(),
                context: String::new(),
            },
            unsupported: None,
        },
    );
    plan
}

pub fn trusted() -> FixRun {
    FixRun {
        trusted: true,
        ..FixRun::default()
    }
}

pub fn text(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join("a.toy")).unwrap()
}
