//! A language that brackets nothing: a class body is a scope the brackets do
//! not show, and the imports lead the file.

use super::*;

const MODULE: &str = "import os\n\ndef fixture():\n    return 1\n\nclass Suite:\n    def test_a(self):\n        return 1\n\n    def test_b(self):\n        return 2\n\ndef helper():\n    return 3\n";

const LATE_IMPORT: &str = "def test_one():\n    return 1\n\nimport os\n\nCONST = os.sep\n";

/// A declaration of `text` over whole lines `from..=to`, in the scope of the
/// class `owner` when it has one.
fn declared(text: &str, owner: Option<&str>, name: &str, (from, to): (u32, u32)) -> Symbol {
    let owners: Vec<&str> = owner.into_iter().collect();
    let kind = SymbolKind::Function;
    Symbol {
        id: SymbolId::new("m", &owners, name, kind),
        owner: owner.map(|o| SymbolId::new("m", &[], o, SymbolKind::Type)),
        ..symbol(text, name, from, to)
    }
}

fn python(text: &str, symbols: Vec<Symbol>) -> Case {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), text).unwrap();
    let project = Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "python".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        symbols,
        ..Fragment::default()
    }]);
    Case { dir, project }
}

fn module() -> Case {
    let class = Symbol {
        id: SymbolId::new("m", &[], "Suite", SymbolKind::Type),
        kind: SymbolKind::Type,
        ..symbol(MODULE, "Suite", 6, 11)
    };
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
    python(
        LATE_IMPORT,
        vec![
            declared(LATE_IMPORT, None, "test_one", (1, 2)),
            declared(LATE_IMPORT, None, "CONST", (6, 6)),
        ],
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

/// The text up to the end of the header of `text` in `language`.
fn header(text: &str, language: &str) -> String {
    text[..super::super::header::end(text, language)].to_owned()
}

#[test]
fn the_header_is_the_imports_that_open_a_line() {
    let text = "\"\"\"Doc.\"\"\"\nfrom __future__ import annotations\nfrom a import (\n    b,\n)\nimport d\n\nX = 1\nif X:\n    import hidden\n";
    assert_eq!(
        header(text, "python"),
        "\"\"\"Doc.\"\"\"\nfrom __future__ import annotations\nfrom a import (\n    b,\n)\nimport d\n"
    );
}

#[test]
fn the_header_of_a_rust_a_typescript_and_a_go_file_ends_with_their_last_import() {
    assert_eq!(
        header(
            "use a::{\n    b,\n};\nuse c;\n\nconst X: i32 = 1;\n",
            "rust"
        ),
        "use a::{\n    b,\n};\nuse c;\n"
    );
    assert_eq!(
        header(
            "import { a } from \"a\";\nimport b from \"b\";\n\nconst x = 1;\n",
            "typescript"
        ),
        "import { a } from \"a\";\nimport b from \"b\";\n"
    );
    assert_eq!(
        header("package p\n\nimport (\n\t\"a\"\n)\n\nconst x = 1\n", "go"),
        "package p\n\nimport (\n\t\"a\"\n)\n"
    );
    assert_eq!(header("import os\n", "cobol"), "");
}
