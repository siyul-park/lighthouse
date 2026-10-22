//! The localized context: the code of the findings, the declarations it uses,
//! its imports and, for the scope `files`, the places that refer to what may
//! change; within a budget, and the same every time.

use std::path::PathBuf;

use lighthouse_model::{
    Diagnostic, Fingerprint, Position, Severity, Span,
    ucm::{
        Edge, EdgeKind, File, Fragment, Module, Node, Project, Resolution, Symbol, SymbolId,
        SymbolKind, Target, Visibility,
    },
};
use lighthouse_plugin::FixDecision;
use lighthouse_spec::FixScope;

use super::{localize, tokens};
use crate::{
    Overlays, Run,
    fix::{
        brief::{Localized, Target as Brief, brief},
        rewrite::World,
    },
};

const BIG: usize = 100_000;

/// One language's fixture: two files, the second of which calls the first.
#[derive(Clone)]
struct Fixture {
    language: &'static str,
    /// The file of the finding, its text, and the declarations in it as
    /// (name, first line, last line, doc).
    own: (&'static str, &'static str),
    own_symbols: [(&'static str, u32, u32, Option<&'static str>); 3],
    /// The line of the call of `own_symbols[1]` to `own_symbols[2]`, of the
    /// import, and the module imported.
    inside: (u32, &'static str),
    /// The file that calls the first symbol, its text, the symbol that calls
    /// and its lines, and the line of the call.
    other: (&'static str, &'static str),
    caller: (&'static str, u32, u32, u32),
}

const GO: Fixture = Fixture {
    language: "go",
    own: (
        "a.go",
        "package m\n\nimport \"fmt\"\n\n// Greet says hello.\nfunc Greet(name string) string {\n\treturn fmt.Sprint(helper(name))\n}\n\nfunc helper(s string) string { return s }\n\nfunc unrelated() {}\n",
    ),
    own_symbols: [
        ("Greet", 6, 8, Some("Greet says hello.")),
        ("helper", 10, 10, None),
        ("unrelated", 12, 12, None),
    ],
    inside: (7, "fmt"),
    other: (
        "b.go",
        "package m\n\nfunc caller() string {\n\treturn Greet(\"x\")\n}\n",
    ),
    caller: ("caller", 3, 5, 4),
};

const RUST: Fixture = Fixture {
    language: "rust",
    own: (
        "a.rs",
        "use std::fmt;\n\n/// Greet says hello.\npub fn greet(name: &str) -> String {\n    format!(\"{}\", helper(name))\n}\n\nfn helper(s: &str) -> String { s.to_owned() }\n\nfn unrelated() {}\n",
    ),
    own_symbols: [
        ("greet", 4, 6, Some("Greet says hello.")),
        ("helper", 8, 8, None),
        ("unrelated", 10, 10, None),
    ],
    inside: (5, "std::fmt"),
    other: ("b.rs", "pub fn caller() -> String {\n    greet(\"x\")\n}\n"),
    caller: ("caller", 1, 3, 2),
};

fn span(from: u32, to: u32) -> Span {
    Span {
        start: Position { line: from, col: 1 },
        end: Position { line: to, col: 2 },
    }
}

fn symbol(file: &str, (name, from, to, doc): (&str, u32, u32, Option<&str>)) -> Symbol {
    Symbol {
        id: SymbolId::new("m", &[], name, SymbolKind::Function),
        kind: SymbolKind::Function,
        visibility: Visibility::Public,
        owner: None,
        file: file.into(),
        span: span(from, to),
        extent: Some(span(from, to)),
        doc: doc.map(str::to_owned),
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

fn file(path: &str, language: &str) -> File {
    File {
        path: path.into(),
        lang: language.to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    }
}

fn calls(from: &Symbol, to: &Symbol, line: u32) -> Edge {
    Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(from.id.clone()),
        to: Target::Resolved(Node::Symbol(to.id.clone())),
        resolution: Resolution::Semantic,
        site: Some(span(line, line)),
    }
}

impl Fixture {
    fn symbols(&self) -> (Vec<Symbol>, Symbol) {
        let own = self.own_symbols.map(|s| symbol(self.own.0, s));
        let (name, from, to, _) = self.caller;
        (own.to_vec(), symbol(self.other.0, (name, from, to, None)))
    }

    /// The project; `reversed` merges its fragments in the other order, `more`
    /// is how many imports the file has besides its own, and `secret` adds an
    /// environment file that holds a symbol the finding uses and one that
    /// calls it.
    fn project(&self, reversed: bool, more: usize, secret: bool) -> Project {
        let (own, caller) = self.symbols();
        let import = |name: String| Edge {
            kind: EdgeKind::Imports,
            from: Node::Module("m".to_owned()),
            to: Target::Path(name),
            resolution: Resolution::Semantic,
            site: None,
        };
        let mut edges = vec![calls(&own[0], &own[1], self.inside.0)];
        edges.push(import(self.inside.1.to_owned()));
        edges.extend((0..more).map(|n| import(format!("example.com/a/very/long/package/{n}"))));
        let mut fragments = vec![
            Fragment {
                files: vec![file(self.own.0, self.language)],
                modules: vec![Module {
                    path: "m".to_owned(),
                    name: None,
                    test_of: None,
                }],
                edges,
                symbols: own.clone(),
                ..Fragment::default()
            },
            Fragment {
                files: vec![file(self.other.0, self.language)],
                edges: vec![calls(&caller, &own[0], self.caller.3)],
                symbols: vec![caller],
                ..Fragment::default()
            },
        ];
        if secret {
            let leak = symbol(".env", ("leak", 1, 1, None));
            fragments.push(Fragment {
                files: vec![file(".env", self.language)],
                edges: vec![
                    calls(&leak, &own[0], 1),
                    calls(&own[0], &leak, self.inside.0),
                ],
                symbols: vec![leak],
                ..Fragment::default()
            });
        }
        if reversed {
            fragments.reverse();
        }
        Project::merge(fragments)
    }

    fn texts(&self) -> Overlays {
        Overlays::from([
            (PathBuf::from(self.own.0), self.own.1.to_owned()),
            (PathBuf::from(self.other.0), self.other.1.to_owned()),
            (PathBuf::from(".env"), "SECRET_TOKEN=abc\n".to_owned()),
        ])
    }

    /// The finding of the first symbol.
    fn finding(&self) -> Diagnostic {
        let (name, from, to, _) = self.own_symbols[0];
        let mut finding = Diagnostic::new(
            "r",
            Severity::Warn,
            "the name is banned",
            self.own.0,
            span(from, to),
            Fingerprint::new("r", self.own.0, name),
        );
        finding.symbol = Some(
            symbol(self.own.0, self.own_symbols[0])
                .id
                .as_str()
                .to_owned(),
        );
        finding
    }

    /// What is localized for the finding at `scope`, within `budget`.
    fn localized(&self, scope: FixScope, budget: usize, reversed: bool) -> Localized {
        self.localized_in(self.project(reversed, 0, false), scope, budget)
    }

    fn localized_in(&self, project: Project, scope: FixScope, budget: usize) -> Localized {
        let outcome = Run {
            project,
            ..Run::default()
        };
        let texts = self.texts();
        let root = PathBuf::from("/nowhere");
        let world = World::new(&root, &outcome, &texts);
        let finding = self.finding();
        let extent =
            (scope == FixScope::Symbol).then(|| span(self.own_symbols[0].1, self.own_symbols[0].2));
        localize(
            &world,
            self.own.0.as_ref(),
            &[(&finding, extent)],
            scope,
            budget,
        )
    }

    /// The brief of the finding, with `localized` in it.
    fn brief(&self, scope: FixScope, localized: &Localized) -> String {
        let decision = FixDecision {
            id: "r".to_owned(),
            requirement: "r requirement".to_owned(),
            context: String::new(),
        };
        let finding = self.finding();
        let target = Brief {
            decision: &decision,
            instruction: None,
            examples: Vec::new(),
            scope,
            message: &finding.message,
            evidence: &finding.evidence,
            place: format!("{}:{}:1", self.own.0, self.own_symbols[0].1),
        };
        brief(&[target], &[], Some(localized))
    }
}

fn fixtures() -> [&'static Fixture; 2] {
    [&GO, &RUST]
}

#[test]
fn the_brief_of_a_symbol_finding_carries_its_code_and_what_it_uses_and_nothing_else() {
    for fixture in fixtures() {
        let localized = fixture.localized(FixScope::Symbol, BIG, false);
        let text = fixture.brief(FixScope::Symbol, &localized);
        let (name, from, to, _) = fixture.own_symbols[0];

        let code: Vec<&str> = fixture.own.1.lines().collect();
        for line in from..=to {
            let numbered = format!("{line:>4}  {}", code[line as usize - 1]);
            assert!(
                text.contains(&numbered),
                "{}: {numbered}\n{text}",
                fixture.language
            );
        }
        assert_eq!(localized.code.len(), 1);
        assert!(
            localized.uses.len() == 1 && localized.uses[0].contains("helper"),
            "{:?}",
            localized.uses
        );
        assert!(
            localized.uses[0]
                .ends_with(&format!("({}:{})", fixture.own.0, fixture.own_symbols[1].1)),
            "{:?}",
            localized.uses
        );
        assert_eq!(
            localized.imports,
            [fixture.inside.1],
            "{}",
            fixture.language
        );
        assert!(
            localized.sites.is_empty() && localized.files.is_empty(),
            "a symbol fix touches no other file: {localized:?}"
        );
        assert_eq!(localized.left_out, 0);
        assert!(text.contains("open further files only when you cannot finish without them"));
        for outside in ["unrelated", "caller(", fixture.other.0] {
            assert!(
                !text.contains(outside),
                "{}: {outside}\n{text}",
                fixture.language
            );
        }
        assert!(text.contains(name));
    }
}

#[test]
fn the_declaration_of_what_a_finding_uses_comes_with_the_first_line_of_its_doc() {
    for fixture in fixtures() {
        let mut documented = fixture.clone();
        documented.own_symbols[1].3 = Some("Helper does the work.\nMore.");
        let localized = documented.localized(FixScope::Symbol, BIG, false);

        assert!(
            localized.uses[0].ends_with(" - Helper does the work."),
            "{:?}",
            localized.uses
        );
    }
}

#[test]
fn the_brief_of_a_files_finding_lists_the_places_that_call_what_may_change() {
    for fixture in fixtures() {
        let localized = fixture.localized(FixScope::Files, BIG, false);
        let text = fixture.brief(FixScope::Files, &localized);

        let call = fixture
            .other
            .1
            .lines()
            .nth(fixture.caller.3 as usize - 1)
            .unwrap();
        assert_eq!(localized.sites.len(), 1, "{:?}", localized.sites);
        assert!(
            localized.sites[0].starts_with(&format!(
                "{}:{}  {}  in ",
                fixture.other.0,
                fixture.caller.3,
                call.trim()
            )) && localized.sites[0].contains("caller"),
            "{:?}",
            localized.sites
        );
        assert_eq!(localized.files, [fixture.other.0]);
        assert!(
            text.contains("Places that refer to what you may change:"),
            "{text}"
        );
        assert!(
            text.contains(&format!("- {}", localized.sites[0])),
            "{text}"
        );
        assert!(text.contains("Files you may have to change:"), "{text}");
    }
}

#[test]
fn what_does_not_fit_the_budget_is_left_out_farthest_first_and_counted() {
    for fixture in fixtures() {
        let all = fixture.localized(FixScope::Files, BIG, false);
        let code: usize = all.code.iter().map(|c| tokens(&c.text)).sum();
        assert_eq!(
            (all.uses.len(), all.imports.len(), all.sites.len()),
            (1, 1, 1)
        );
        assert_eq!(all.left_out, 0);

        // The code of the finding is always given; what is left over takes the
        // declarations and the imports before the places that call it.
        let near = code + tokens(&all.uses[0]) + tokens(&all.imports[0]);
        let some = fixture.localized(FixScope::Files, near, false);
        assert_eq!(
            (
                some.uses.len(),
                some.imports.len(),
                some.sites.len(),
                some.left_out
            ),
            (1, 1, 0, 1),
            "{}",
            fixture.language
        );
        assert_eq!(
            some.files,
            [fixture.other.0],
            "named for every place, kept or not"
        );

        let none = fixture.localized(FixScope::Files, 0, false);
        assert_eq!(none.code, all.code, "the code is never left out");
        assert_eq!(
            (none.uses.len(), none.imports.len(), none.sites.len()),
            (0, 0, 0)
        );
        assert_eq!(none.left_out, 3);
        assert_eq!(none.files, [fixture.other.0]);
        let text = fixture.brief(FixScope::Files, &none);
        assert!(
            text.contains("3 more items were left out to keep this short."),
            "{text}"
        );
    }
}

#[test]
fn the_same_inputs_make_the_same_brief_whatever_order_the_project_was_merged_in() {
    for fixture in fixtures() {
        let first = fixture.localized(FixScope::Files, BIG, false);
        let second = fixture.localized(FixScope::Files, BIG, true);

        assert_eq!(first, second);
        assert_eq!(
            fixture.brief(FixScope::Files, &first),
            fixture.brief(FixScope::Files, &second)
        );
    }
}

#[test]
fn a_long_list_of_imports_takes_what_is_left_and_drops_no_signature_or_place() {
    for fixture in fixtures() {
        let all = fixture.localized(FixScope::Files, BIG, false);
        let code: usize = all.code.iter().map(|c| tokens(&c.text)).sum();
        let kept = tokens(&all.uses[0]) + tokens(&all.sites[0]) + 20;

        let long = fixture.localized_in(
            fixture.project(false, 300, false),
            FixScope::Files,
            code + kept,
        );

        assert_eq!(
            (long.uses.len(), long.sites.len()),
            (1, 1),
            "{}",
            fixture.language
        );
        assert!(
            !long.imports.is_empty() && long.imports.len() < 301,
            "{:?}",
            long.imports
        );
        assert_eq!(long.left_out, 301 - long.imports.len());
    }
}

#[test]
fn a_symbol_inside_the_extent_of_another_is_not_given_twice() {
    for fixture in fixtures() {
        let outcome = Run {
            project: fixture.project(false, 0, false),
            ..Run::default()
        };
        let texts = fixture.texts();
        let root = PathBuf::from("/nowhere");
        let world = World::new(&root, &outcome, &texts);
        let finding = fixture.finding();
        let (_, from, to, _) = fixture.own_symbols[0];

        let localized = localize(
            &world,
            fixture.own.0.as_ref(),
            &[
                (&finding, Some(span(from + 1, from + 1))),
                (&finding, Some(span(from, to))),
                (&finding, Some(span(from, to))),
            ],
            FixScope::Symbol,
            BIG,
        );

        assert_eq!(localized.code.len(), 1, "{}", fixture.language);
        assert_eq!(localized.code[0].first, from);
    }
}

#[test]
fn a_brief_never_carries_a_file_the_scratch_copy_leaves_out() {
    for fixture in fixtures() {
        for scope in [FixScope::Symbol, FixScope::Files] {
            let localized = fixture.localized_in(fixture.project(false, 0, true), scope, BIG);
            let text = fixture.brief(scope, &localized);

            assert!(
                !text.contains("SECRET_TOKEN") && !text.contains(".env") && !text.contains("leak"),
                "{}: {text}",
                fixture.language
            );
        }
    }
}
