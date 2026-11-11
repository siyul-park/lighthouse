//! A toy language and rules for the fix tests: lines of `fn name` are symbols,
//! `TODO` and `BAD` are findings, and `BROKEN` makes a file unanalyzable.
#![allow(dead_code)]

use std::{fs, sync::LazyLock};

use lighthouse_engine::{Engine, FixBinding, FixPlan, FixedBy, Needs};
use lighthouse_model::{
    Diagnostic, Fingerprint, Incomplete, LineIndex, Options, Position, Safety, Severity, Span,
    ucm::{
        Capability, Edge, EdgeKind, EditOp, File, FixOutcome, Fragment, Node, Place, Resolution,
        Symbol, SymbolId, SymbolKind, Target, Visibility,
    },
};
use lighthouse_model::{RunScope, ucm::Applicability};
use lighthouse_plugin::{
    Context, Error as PluginError, FixDecision, FixRequest, Fixer, FixerManifest, Fragments,
    Indexed, LanguageManifest, LanguageProvider, Plugin, PluginManifest, Registry, Rule,
    RuleManifest, Unit, Workspace,
};
use lighthouse_spec::Config;
use tempfile::TempDir;

/// The language manifest of the toy language: it analyzes overlays.
pub fn toy() -> LanguageManifest {
    LanguageManifest {
        capabilities: vec![lighthouse_model::ucm::Capability::Overlays],
        ordered_declarations: true,
        ..LanguageManifest::new("toy", vec!["**/*.toy".to_owned()])
    }
}

pub struct Toy(pub LanguageManifest);

impl LanguageProvider for Toy {
    fn manifest(&self) -> &LanguageManifest {
        &self.0
    }

    fn index(&self, _: &Workspace, units: &[Unit], _: &[String]) -> Result<Indexed, PluginError> {
        let mut out = Indexed::default();
        for unit in units {
            let mut indexed = Fragments::new(unit.id, Vec::new());
            for source in unit.files {
                let (symbols, places): (Vec<_>, Vec<_>) = source
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
                        let id = SymbolId::new("m", &[], name, SymbolKind::Function);
                        let place = Place {
                            extent: Some(span),
                            ..Place::new(id.clone(), span)
                        };
                        let symbol = Symbol {
                            id,
                            kind: SymbolKind::Function,
                            visibility: Visibility::Private,
                            owner: None,
                            file: source.file.path.clone(),
                            name: name.to_owned(),
                            role: None,
                            optional: false,
                            type_ref: None,
                            embedded: false,
                            implementation: false,
                        };
                        Some((symbol, place))
                    })
                    .unzip();
                // `call name` refers to the symbol `name` from the function above it;
                // `load name` does too, as the file loads.
                let mut edges = Vec::new();
                let mut current = None;
                for (n, line) in source.text.lines().enumerate() {
                    if let Some(name) = line
                        .strip_prefix("fn ")
                        .and_then(|l| l.split_whitespace().next())
                    {
                        current = Some(SymbolId::new("m", &[], name, SymbolKind::Function));
                    }
                    let loads = line.starts_with("load ");
                    let target = line
                        .strip_prefix("call ")
                        .or_else(|| line.strip_prefix("load "))
                        .and_then(|l| l.split_whitespace().next());
                    if let (Some(from), Some(name)) = (&current, target) {
                        let at = |col: usize| Position {
                            line: u32::try_from(n + 1).unwrap(),
                            col: u32::try_from(col + 1).unwrap(),
                        };
                        edges.push(Edge {
                            kind: if loads {
                                EdgeKind::References
                            } else {
                                EdgeKind::Calls
                            },
                            from: Node::Symbol(from.clone()),
                            to: Target::Resolved(Node::Symbol(SymbolId::new(
                                "m",
                                &[],
                                name,
                                SymbolKind::Function,
                            ))),
                            resolution: Resolution::Semantic,
                            site: Some(Span {
                                start: at(0),
                                end: at(line.len()),
                            }),
                            when: if loads {
                                lighthouse_model::ucm::When::Load
                            } else {
                                Default::default()
                            },
                        });
                    }
                }
                if source.text.contains("BROKEN") {
                    indexed.incomplete.push(Incomplete {
                        path: Some(source.file.path.clone()),
                        provider: None,
                        reason: "cannot parse".to_owned(),
                    });
                }
                if source.text.contains("PARTIAL") {
                    indexed.incomplete.push(Incomplete {
                        path: None,
                        provider: None,
                        reason: "the provider stopped".to_owned(),
                    });
                }
                indexed.fragments.push(Fragment {
                    files: vec![File {
                        ..source.file.clone()
                    }],
                    symbols,
                    places,
                    edges,
                    ..Fragment::default()
                });
            }
            out.units.push(indexed);
        }
        Ok(out)
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
                revision: None,
                tools: Vec::new(),
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

    fn check(&self, ctx: &Context, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let (file, text) = ctx
            .file
            .ok_or_else(|| PluginError::Failed("no file".into()))?;
        let index = LineIndex::new(text.get());
        Ok(text
            .get()
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
                    Fingerprint::new(&self.manifest.id, &file.path.to_string_lossy(), "")
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

/// Rewrites the finding and then the lines it is given, whole, in any file: a
/// change of a declaration and of the places that refer to it.
pub struct Spread {
    manifest: FixerManifest,
    lines: &'static [(&'static str, u32, &'static str, &'static str)],
}

impl Fixer for Spread {
    fn manifest(&self) -> &FixerManifest {
        &self.manifest
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        let line = |file: &str, n: u32, old: &str, text: &str| EditOp::Replace {
            file: file.into(),
            span: Span {
                start: Position { line: n, col: 1 },
                end: Position {
                    line: n,
                    col: u32::try_from(old.len() + 1).unwrap(),
                },
            },
            text: text.to_owned(),
        };
        let mut ops = vec![EditOp::Replace {
            file: request.finding.file.clone(),
            span: request.finding.span,
            text: "DONE".to_owned(),
        }];
        ops.extend(
            self.lines
                .iter()
                .map(|(file, n, old, text)| line(file, *n, old, text)),
        );
        Ok(FixOutcome::Proposed {
            description: "change a declaration and its places".to_owned(),
            ops,
            safety: Safety::Safe,
        })
    }
}

pub fn spread(
    id: &str,
    lines: &'static [(&'static str, u32, &'static str, &'static str)],
) -> Spread {
    Spread {
        manifest: FixerManifest {
            id: id.to_owned(),
            requires: Vec::new(),
        },
        lines,
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
            Box::new(spread(
                "fake/callers",
                &[
                    ("a.toy", 1, "fn a", "fn a 2"),
                    ("b.toy", 2, "call a", "call a 2"),
                    ("c.toy", 2, "call a", "call a 2"),
                ],
            )),
            Box::new(spread(
                "fake/transitive",
                &[
                    ("a.toy", 1, "fn a", "fn a 2"),
                    ("b.toy", 1, "fn b", "fn b 2"),
                    ("e.toy", 2, "call b", "call b 2"),
                ],
            )),
            Box::new(spread(
                "fake/unrelated",
                &[
                    ("a.toy", 1, "fn a", "fn a 2"),
                    ("d.toy", 1, "fn d", "fn d 2"),
                ],
            )),
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
    Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap()
}

pub fn project(text: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.toy"), text).unwrap();
    dir
}

pub fn plan(fixer: &str, cap: Safety) -> FixPlan {
    let mut plan = FixPlan::default();
    plan.insert(
        "fake/todo",
        FixBinding {
            fixer: FixedBy::Fixer(fixer.to_owned()),
            cap,
            languages: Vec::new(),
            decision: FixDecision {
                id: "fake/todo".to_owned(),
                requirement: String::new(),
                context: String::new(),
            },
            needs: Needs::Nothing,
        },
    );
    plan
}

/// A project of `a.toy`, whose `a` is called by `b` in `b.toy` and `c` in
/// `c.toy`; `b` is called by `e` in `e.toy`; `d.toy` refers to nothing.
pub fn callers() -> TempDir {
    let dir = project("fn a\nTODO here\n");
    for (file, text) in [
        ("b.toy", "fn b\ncall a\n"),
        ("c.toy", "fn c\ncall a\n"),
        ("d.toy", "fn d\n"),
        ("e.toy", "fn e\ncall b\n"),
    ] {
        fs::write(dir.path().join(file), text).unwrap();
    }
    dir
}

pub fn read(dir: &TempDir, file: &str) -> String {
    fs::read_to_string(dir.path().join(file)).unwrap()
}

pub fn text(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join("a.toy")).unwrap()
}

/// How many whole checks each round of a fix run made, as (round, checks).
pub fn checks_per_round(report: &lighthouse_engine::FixReport) -> Vec<(usize, usize)> {
    let mut counts: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for (round, _) in &report.timings.checks {
        *counts.entry(*round).or_default() += 1;
    }
    counts.into_iter().collect()
}
