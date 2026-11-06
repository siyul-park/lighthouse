//! A language that brackets nothing: a class body is a scope the brackets do
//! not show, and the imports lead the file.

use super::*;
use lighthouse_model::ucm::{Edge, EdgeKind, Node, Place, Resolution, Target};

const MODULE: &str = "import os\n\ndef fixture():\n    return 1\n\nclass Suite:\n    def test_a(self):\n        return 1\n\n    def test_b(self):\n        return 2\n\ndef helper():\n    return 3\n";

const LATE_IMPORT: &str = "def test_one():\n    return 1\n\nimport os\n\nCONST = os.sep\n";

/// A declaration of `text` over whole lines `from..=to`, in the scope of the
/// class `owner` when it has one.
fn declared(
    text: &str,
    owner: Option<&str>,
    name: &str,
    (from, to): (u32, u32),
) -> (Symbol, Place) {
    let owners: Vec<&str> = owner.into_iter().collect();
    let kind = SymbolKind::Function;
    let (shaped, placed) = symbol(text, name, from, to);
    let id = SymbolId::new("m", &owners, name, kind);
    let symbol = Symbol {
        id: id.clone(),
        owner: owner.map(|o| SymbolId::new("m", &[], o, SymbolKind::Type)),
        ..shaped
    };
    (
        symbol,
        Place {
            symbol: id,
            ..placed
        },
    )
}

/// An import of the module `m` on `line` of `text`, at the whole line.
fn import(text: &str, line: u32) -> Edge {
    let width = text.lines().nth(line as usize - 1).map_or(0, str::len);
    Edge {
        kind: EdgeKind::Imports,
        from: Node::Module("m".to_owned()),
        to: Target::Path("os".to_owned()),
        resolution: Resolution::Syntactic,
        site: Some(span((line, 1), (line, u32::try_from(width).unwrap() + 1))),
        when: Default::default(),
    }
}

fn python(text: &str, symbols: Vec<(Symbol, Place)>) -> Case {
    python_with(text, symbols, Vec::new())
}

fn python_with(text: &str, declared: Vec<(Symbol, Place)>, edges: Vec<Edge>) -> Case {
    let (symbols, places) = declared.into_iter().unzip();
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), text).unwrap();
    let project = Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "python".to_owned(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        symbols,
        places,
        edges,
        ..Fragment::default()
    }]);
    Case { dir, project }
}

fn module() -> Case {
    let (shaped, placed) = symbol(MODULE, "Suite", 6, 11);
    let id = SymbolId::new("m", &[], "Suite", SymbolKind::Type);
    let class = (
        Symbol {
            id: id.clone(),
            kind: SymbolKind::Type,
            ..shaped
        },
        Place {
            symbol: id,
            ..placed
        },
    );
    python(
        MODULE,
        vec![
            declared(MODULE, None, "fixture", (3, 4)),
            class,
            declared(MODULE, Some("Suite"), "test_a", (7, 8)),
            declared(MODULE, Some("Suite"), "test_b", (10, 11)),
            declared(MODULE, None, "helper", (13, 14)),
        ],
    )
}

fn method(name: &str) -> Node {
    Node::Symbol(SymbolId::new("m", &["Suite"], name, SymbolKind::Function))
}

#[test]
fn a_function_is_not_moved_next_to_a_method_of_a_class() {
    let why = module()
        .run(&[EditOp::Move {
            node: id("fixture"),
            anchor: Anchor::Before(method("test_a")),
        }])
        .unwrap_err();

    assert!(why.contains("not in the same block"), "{why}");
}

#[test]
fn a_method_is_not_moved_next_to_a_function() {
    let why = module()
        .run(&[EditOp::Move {
            node: method("test_b"),
            anchor: Anchor::After(id("helper")),
        }])
        .unwrap_err();

    assert!(why.contains("not in the same block"), "{why}");
}

#[test]
fn methods_and_functions_listed_together_are_ordered_each_among_its_own() {
    let moved = module()
        .run(&[EditOp::Reorder {
            owner: Owner::File(FILE.into()),
            order: vec![
                id("helper"),
                method("test_b"),
                id("fixture"),
                method("test_a"),
            ],
        }])
        .unwrap();

    assert_eq!(
        moved,
        "import os\n\nclass Suite:\n    def test_b(self):\n        return 2\n\n    def test_a(self):\n        return 1\n\ndef helper():\n    return 3\n\ndef fixture():\n    return 1\n"
    );
}

fn late() -> Case {
    python_with(
        LATE_IMPORT,
        vec![
            declared(LATE_IMPORT, None, "test_one", (1, 2)),
            declared(LATE_IMPORT, None, "CONST", (6, 6)),
        ],
        vec![import(LATE_IMPORT, 4)],
    )
}

#[test]
fn a_declaration_is_not_put_above_an_import() {
    let why = late()
        .run(&[EditOp::Move {
            node: id("CONST"),
            anchor: Anchor::Before(id("test_one")),
        }])
        .unwrap_err();

    assert!(why.contains("above the imports"), "{why}");
}

#[test]
fn a_declaration_is_put_below_the_imports_it_follows() {
    let moved = late()
        .run(&[EditOp::Move {
            node: id("test_one"),
            anchor: Anchor::After(id("CONST")),
        }])
        .unwrap();

    assert_eq!(
        moved,
        "import os\n\nCONST = os.sep\n\ndef test_one():\n    return 1\n"
    );
}

const LOCAL_IMPORT: &str =
    "def first():\n    import json\n    return json\n\ndef second():\n    return 2\n";

#[test]
fn an_import_inside_a_function_is_no_module_level_import() {
    let case = python_with(
        LOCAL_IMPORT,
        vec![
            declared(LOCAL_IMPORT, None, "first", (1, 3)),
            declared(LOCAL_IMPORT, None, "second", (5, 6)),
        ],
        vec![import(LOCAL_IMPORT, 2)],
    );

    let moved = case
        .run(&[EditOp::Move {
            node: id("second"),
            anchor: Anchor::Before(id("first")),
        }])
        .unwrap();

    assert_eq!(
        moved,
        "def second():\n    return 2\n\ndef first():\n    import json\n    return json\n"
    );
}
