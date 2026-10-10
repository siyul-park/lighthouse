//! A synthetic project: symbols, edges and facts written directly in the code
//! model, checked through the whole engine with one rule.
#![allow(dead_code)]

use std::{collections::BTreeMap, fs};

use lighthouse_checks::{Pack, metrics::Metrics};
use lighthouse_engine::Engine;
use lighthouse_model::{
    Comment, Edge, EdgeKind, File, Flow, Fragment, FunctionSummary, Module, Node, Position,
    Resolution, Span, Symbol, SymbolId, SymbolKind, Target, TestCase, TestStyle, Visibility,
};
use lighthouse_plugin::{
    Conventions, Error, Indexed, LanguageProvider, Plugin, PluginManifest, ProviderManifest,
    Registry, Source, Workspace,
};
use lighthouse_spec::Catalog;
use lighthouse_spec::Config;
use serde_json::Value;

/// Reads a UCM fragment as JSON, the way an out-of-process provider would send it.
struct Wire(ProviderManifest);

impl LanguageProvider for Wire {
    fn manifest(&self) -> &ProviderManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        let fragments = files
            .iter()
            .map(|source| {
                serde_json::from_str(source.text).map_err(|e| Error::Failed(e.to_string()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Indexed {
            fragments,
            ..Indexed::default()
        })
    }
}

struct Fixture(PluginManifest);

impl Fixture {
    fn new() -> Self {
        Self(PluginManifest {
            id: "fixture".to_owned(),
            version: "0".to_owned(),
        })
    }
}

impl Plugin for Fixture {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Wire(ProviderManifest {
            conventions: Conventions {
                test_globs: vec!["**/*_test.ucm".to_owned()],
                constructor_prefixes: vec!["New".to_owned(), "new".to_owned()],
            },
            ..ProviderManifest::new("wire", vec!["**/*.ucm".to_owned()])
        }))]
    }
}

/// The plugin under test, shared by the tests of one crate.
pub struct Subject<'a> {
    pub plugin: &'a dyn Plugin,
    pub id: &'a str,
}

#[derive(Default)]
pub struct World {
    pub files: BTreeMap<String, bool>,
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub summaries: Vec<FunctionSummary>,
    pub comments: Vec<Comment>,
    pub tests: Vec<TestCase>,
    pub modules: Vec<Module>,
    /// `(importer, imported)` module paths, as import edges of the model.
    pub imports: Vec<(String, String)>,
}

pub fn at(line: u32) -> Span {
    let p = Position { line, col: 1 };
    Span { start: p, end: p }
}

impl World {
    pub fn symbol(&mut self, module: &str, name: &str, kind: SymbolKind, file: &str) -> Symbol {
        self.files.entry(file.to_owned()).or_insert(false);
        let line = u32::try_from(self.symbols.len()).unwrap() + 1;
        let symbol = Symbol {
            id: SymbolId::new(module, &[], name, kind),
            kind,
            visibility: Visibility::Public,
            owner: None,
            file: file.into(),
            span: at(line),
            extent: None,
            doc: None,
            name: name.to_owned(),
            role: None,
            optional: false,
        };
        self.symbols.push(symbol.clone());
        symbol
    }

    /// A member of `owner`, such as a method.
    pub fn member(&mut self, owner: &Symbol, name: &str, kind: SymbolKind, file: &str) -> Symbol {
        let mut symbol = self.symbol(owner.id.module(), name, kind, file);
        symbol.id = SymbolId::new(owner.id.module(), &[owner.name.as_str()], name, kind);
        symbol.owner = Some(owner.id.clone());
        *self.symbols.last_mut().unwrap() = symbol.clone();
        if matches!(kind, SymbolKind::Method | SymbolKind::Function) {
            self.summarize(&symbol, 1, &[]);
        }
        symbol
    }

    pub fn func(&mut self, module: &str, name: &str, file: &str) -> Symbol {
        let symbol = self.symbol(module, name, SymbolKind::Function, file);
        self.summarize(&symbol, 1, &[]);
        symbol
    }

    pub fn private(&mut self, symbol: &Symbol) {
        self.symbols
            .iter_mut()
            .find(|s| s.id == symbol.id)
            .unwrap()
            .visibility = Visibility::Private;
    }

    pub fn summarize(&mut self, symbol: &Symbol, statements: u32, flow: &[Flow]) {
        self.summaries.retain(|s| s.symbol != symbol.id);
        self.summaries.push(FunctionSummary {
            symbol: symbol.id.clone(),
            max_nesting: flow.iter().map(|f| f.nesting + 1).max().unwrap_or(0),
            statements,
            top_level: 1,
            params: 0,
            returns: 0,
            tokens: 0,
            flow: flow.to_vec(),
            clone_fingerprint: None,
            forwards_to: None,
            param_types: Vec::new(),
            result_types: Vec::new(),
            manual_assertions: 0,
            implementation: false,
            constructs: false,
        });
    }

    pub fn edge(&mut self, kind: EdgeKind, from: &Symbol, to: &Symbol) {
        self.edges.push(Edge {
            kind,
            from: Node::Symbol(from.id.clone()),
            to: Target::Path(to.id.as_str().to_owned()),
            resolution: Resolution::Syntactic,
            site: None,
        });
    }

    /// Module `from` imports module `to`.
    pub fn import(&mut self, from: &str, to: &str) {
        self.imports.push((from.to_owned(), to.to_owned()));
    }

    pub fn forward(&mut self, from: &Symbol, to: &Symbol) {
        let summary = self
            .summaries
            .iter_mut()
            .find(|s| s.symbol == from.id)
            .unwrap();
        summary.forwards_to = Some(Target::Path(to.id.as_str().to_owned()));
    }

    /// A comment on `line` of `file`.
    pub fn comment(&mut self, file: &str, line: u32, text: &str) {
        self.files.entry(file.to_owned()).or_insert(false);
        self.comments.push(Comment {
            file: file.into(),
            span: at(line),
            text: text.to_owned(),
            attached_to: None,
        });
    }

    /// A test case whose targets are `targets`.
    pub fn test_case(&mut self, test: &Symbol, targets: &[&Symbol]) {
        self.tests.push(TestCase {
            symbol: test.id.clone(),
            nesting: 0,
            style: TestStyle::Scenario,
            targets: targets
                .iter()
                .map(|t| Target::Path(t.id.as_str().to_owned()))
                .collect(),
        });
    }

    /// Declares module `path` with the name the language gives it.
    pub fn module(&mut self, path: &str, name: Option<&str>, test_of: Option<&str>) {
        self.modules.push(Module {
            path: path.to_owned(),
            name: name.map(str::to_owned),
            test_of: test_of.map(str::to_owned),
        });
    }

    pub fn check(&self, rule: &str, options: Value) -> Vec<(String, u32)> {
        let design = Pack::of("design", Catalog::bundled());
        self.check_in(
            &Subject {
                plugin: &design,
                id: "design",
            },
            rule,
            options,
        )
    }

    pub fn check_in(&self, subject: &Subject, rule: &str, options: Value) -> Vec<(String, u32)> {
        let dir = tempfile::tempdir().unwrap();
        for (path, generated) in &self.files {
            let file = File {
                path: path.into(),
                lang: "wire".to_owned(),
                hash: String::new(),
                generated: *generated,
                test: path.ends_with("_test.ucm"),
            };
            let own = |s: &Symbol| s.file.to_string_lossy() == path.as_str();
            let ids: Vec<&SymbolId> = self
                .symbols
                .iter()
                .filter(|s| own(s))
                .map(|s| &s.id)
                .collect();
            let fragment = Fragment {
                files: vec![file],
                modules: self.modules.clone(),
                symbols: self.symbols.iter().filter(|s| own(s)).cloned().collect(),
                functions: self
                    .summaries
                    .iter()
                    .filter(|f| ids.contains(&&f.symbol))
                    .cloned()
                    .collect(),
                edges: self
                    .edges
                    .iter()
                    .filter(|e| matches!(&e.from, Node::Symbol(id) if ids.contains(&id)))
                    .cloned()
                    .chain(
                        self.imports
                            .iter()
                            .filter(|(from, _)| ids.iter().any(|id| id.module() == from))
                            .map(|(from, to)| Edge {
                                kind: EdgeKind::Imports,
                                from: Node::Module(from.clone()),
                                to: Target::Path(to.clone()),
                                resolution: Resolution::Syntactic,
                                site: None,
                            }),
                    )
                    .collect(),
                tests: self
                    .tests
                    .iter()
                    .filter(|t| ids.contains(&&t.symbol))
                    .cloned()
                    .collect(),
                comments: self
                    .comments
                    .iter()
                    .filter(|c| c.file.to_string_lossy() == path.as_str())
                    .cloned()
                    .collect(),
            };
            let target = dir.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, serde_json::to_string(&fragment).unwrap()).unwrap();
        }
        let mut level = toml::Table::new();
        level.insert("level".to_owned(), "warn".into());
        let mut settings = toml::Table::new();
        for (key, value) in options.as_object().unwrap() {
            settings.insert(key.clone(), toml::Value::try_from(value).unwrap());
        }
        if !settings.is_empty() {
            level.insert("options".to_owned(), settings.into());
        }
        let mut rules = toml::Table::new();
        rules.insert(rule.to_owned(), level.into());
        let mut root = toml::Table::new();
        root.insert(
            "plugins".to_owned(),
            vec!["fixture", "metrics", subject.id].into(),
        );
        root.insert("rules".to_owned(), rules.into());
        let config = Config::parse_inline(&root.to_string()).unwrap();
        let mut registry = Registry::default();
        registry.register(&Fixture::new()).unwrap();
        registry.register(&Metrics).unwrap();
        registry.register(subject.plugin).unwrap();
        let outcome = Engine::new(registry, config, Catalog::bundled(), dir.path())
            .unwrap()
            .check(&[], &[rule.to_owned()])
            .unwrap();
        assert!(outcome.notices.is_empty(), "{:?}", outcome.notices);
        assert!(
            outcome.incomplete.is_empty(),
            "the check did not finish: {:?}",
            outcome.incomplete
        );
        outcome
            .diagnostics
            .iter()
            .map(|d| (d.file.to_string_lossy().into_owned(), d.span.start.line))
            .collect()
    }

    pub fn names(&self, found: &[(String, u32)]) -> Vec<String> {
        found
            .iter()
            .map(|(file, line)| {
                self.symbols
                    .iter()
                    .find(|s| {
                        s.file.to_string_lossy() == file.as_str() && s.span.start.line == *line
                    })
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| format!("{file}:{line}"))
            })
            .collect()
    }
}
