use lighthouse_model::{
    Position, Span,
    ucm::{File, Fragment, Module, Symbol, SymbolId, SymbolKind, TestCase, TestStyle},
};

use super::*;
use crate::fix::edits::TextEdit;

fn span() -> Span {
    let at = Position { line: 1, col: 1 };
    Span { start: at, end: at }
}

fn symbol(owners: &[&str], name: &str, kind: SymbolKind) -> Symbol {
    let mut symbol = Symbol::new(
        SymbolId::new("m", owners, name, kind),
        kind,
        Visibility::Public,
        "m.py".into(),
        span(),
        name,
    );
    symbol.owner = owners
        .last()
        .map(|owner| SymbolId::new("m", &owners[..owners.len() - 1], owner, SymbolKind::Type));
    symbol
}

fn project(symbols: Vec<Symbol>, tests: &[&str]) -> Project {
    let tests = tests
        .iter()
        .filter_map(|name| symbols.iter().find(|s| s.name == *name))
        .map(|s| TestCase {
            symbol: s.id.clone(),
            nesting: 0,
            style: TestStyle::Scenario,
            targets: Vec::new(),
        })
        .collect();
    Project::merge([Fragment {
        files: vec![File {
            path: "m.py".into(),
            lang: "python".to_owned(),
            hash: String::new(),
            generated: false,
            test: true,
            facts: Default::default(),
        }],
        modules: vec![Module {
            path: "m".to_owned(),
            name: None,
            test_of: None,
            language: None,
            example: false,
        }],
        symbols,
        tests,
        ..Default::default()
    }])
}

fn changes(before: &Project, after: &Project) -> Option<String> {
    let file = PathBuf::from("m.py");
    let files = BTreeSet::from([&file]);
    displaced(&files, (before, after)).remove(&file)
}

fn class() -> Symbol {
    symbol(&[], "Suite", SymbolKind::Type)
}

#[test]
fn a_function_put_in_a_class_is_a_change_of_its_owner() {
    let before = project(
        vec![class(), symbol(&[], "fixture", SymbolKind::Function)],
        &[],
    );
    let after = project(
        vec![class(), symbol(&["Suite"], "fixture", SymbolKind::Function)],
        &[],
    );
    assert_eq!(
        changes(&before, &after).as_deref(),
        Some("the move would change `fixture`: owner the file → `Suite`")
    );
}

#[test]
fn a_method_that_leaves_for_dead_code_is_dropped() {
    let before = project(
        vec![class(), symbol(&["Suite"], "test_run", SymbolKind::Test)],
        &["test_run"],
    );
    let after = project(vec![class()], &[]);
    assert_eq!(
        changes(&before, &after).as_deref(),
        Some("the move would drop `test_run`")
    );
}

#[test]
fn a_file_with_the_same_symbols_and_tests_is_not_displaced() {
    let symbols = || vec![class(), symbol(&["Suite"], "test_run", SymbolKind::Test)];
    assert_eq!(
        changes(
            &project(symbols(), &["test_run"]),
            &project(symbols(), &["test_run"])
        ),
        None
    );
}

#[test]
fn a_test_that_stops_being_a_test_case_is_a_change() {
    let symbols = || vec![class(), symbol(&["Suite"], "test_run", SymbolKind::Test)];
    assert_eq!(
        changes(&project(symbols(), &["test_run"]), &project(symbols(), &[])).as_deref(),
        Some("the move would change `test_run`: its visibility or test case")
    );
}

#[test]
fn only_the_files_that_only_move_are_judged() {
    let mut moving = Vec::new();
    let mut other = Vec::new();
    for (key, moves_only) in [("1", true), ("2", false)] {
        let edit = TextEdit::insert(format!("{key}.py").into(), 0, String::new());
        let mut p = proposal(key, vec![edit]);
        p.moves_only = moves_only;
        if moves_only {
            moving.push(p);
        } else {
            other.push(p);
        }
    }
    let all: Vec<Proposal> = moving.into_iter().chain(other).collect();
    assert_eq!(
        moved_files(&all).into_iter().collect::<Vec<_>>(),
        [&PathBuf::from("1.py")]
    );
}

fn proposal(key: &str, edits: Vec<TextEdit>) -> Proposal {
    use crate::fix::{FixedBy, propose::Aim};
    use lighthouse_model::{Fingerprint, Safety};
    Proposal {
        fingerprint: Fingerprint::new("r", key, key),
        rule_id: "r".to_owned(),
        file: "m.py".into(),
        line: 1,
        fixer: FixedBy::Fixer("tidy".to_owned()),
        description: "tidy".to_owned(),
        safety: Safety::Safe,
        priority: 0,
        files: edits.iter().map(|e| e.file.clone()).collect(),
        edits,
        aim: Aim::Finding,
        chain: 0,
        bases: BTreeMap::new(),
        batch: Vec::new(),
        moves_only: true,
    }
}

#[test]
fn symbols_that_share_an_id_are_compared_as_a_set() {
    let twice = || {
        vec![
            symbol(&[], "init", SymbolKind::Function),
            symbol(&[], "init", SymbolKind::Function),
        ]
    };
    assert_eq!(
        changes(&project(twice(), &[]), &project(twice(), &[])),
        None
    );
}
