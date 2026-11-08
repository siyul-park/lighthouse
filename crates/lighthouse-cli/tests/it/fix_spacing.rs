//! The blank lines after a fix that moves declarations: each gap keeps the
//! separation of its file and scope, comments, decorators and attributes
//! travel with their declaration, the file ends as it did, and a second fix
//! changes nothing. Over the real language plugins.

use predicates::prelude::*;
use tempfile::TempDir;

use crate::{
    fix::{go_project, lighthouse, read, rust_project},
    fix_header::{project, python},
};

/// Fixes `decision` in `dir` twice, and returns the text of `file` after the
/// first; the second changes nothing.
fn fixed(dir: &TempDir, decision: &str, file: &str) -> String {
    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", decision])
        .assert()
        .stderr(predicate::str::contains("rolled back").not());
    let once = read(dir, file);
    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", decision])
        .assert()
        .stderr(predicate::str::contains("fixed ").not());
    assert_eq!(read(dir, file), once, "the second fix is a no-op");
    once
}

const SHAPES: &str = "def area(side):\n    return side * side\n";

const PYTHON: &str = "import pytest\n\nfrom shapes import area\n\n\ndef test_area():\n    assert area(build()) == 9\n\n\ndef test_again():\n    assert area(WIDTH) == 9\n\n\n# builds.\n@pytest.fixture\ndef build():\n    return 3\n\n\nWIDTH = 3\n";
const PYTHON_FIXED: &str = "import pytest\n\nfrom shapes import area\n\n\nWIDTH = 3\n\n\n# builds.\n@pytest.fixture\ndef build():\n    return 3\n\n\ndef test_area():\n    assert area(build()) == 9\n\n\ndef test_again():\n    assert area(WIDTH) == 9\n";

#[test]
fn python_keeps_two_blank_lines_a_decorator_and_a_comment_with_its_fixture() {
    let Some(dir) = python(&[("shapes.py", SHAPES), ("test_shapes.py", PYTHON)]) else {
        return;
    };

    assert_eq!(
        fixed(&dir, "testing/file-layout", "test_shapes.py"),
        PYTHON_FIXED
    );
}

#[test]
fn python_keeps_crlf_line_ends_and_the_separation() {
    let crlf = |text: &str| text.replace('\n', "\r\n");
    let Some(dir) = python(&[("shapes.py", SHAPES), ("test_shapes.py", &crlf(PYTHON))]) else {
        return;
    };

    assert_eq!(
        fixed(&dir, "testing/file-layout", "test_shapes.py"),
        crlf(PYTHON_FIXED)
    );
}

#[test]
fn python_keeps_a_last_line_without_a_newline_without_one() {
    let source = PYTHON.trim_end_matches('\n');
    let Some(dir) = python(&[("shapes.py", SHAPES), ("test_shapes.py", source)]) else {
        return;
    };

    assert_eq!(
        fixed(&dir, "testing/file-layout", "test_shapes.py"),
        PYTHON_FIXED.trim_end_matches('\n')
    );
}

const TS_SOURCE: &str = "export function area(n: number): number {\n  return n * n;\n}\n";
const TS: &str = "import { it, expect } from \"vitest\";\nimport { area } from \"./svc\";\n\nit(\"squares\", () => {\n  expect(area(WIDTH)).toBe(4);\n});\n\n\n// The width.\nexport const WIDTH = 2;\n\nfunction build(): number {\n  return 1;\n}\n\nit(\"builds\", () => {\n  expect(build()).toBe(1);\n});\n";
const TS_FIXED: &str = "import { it, expect } from \"vitest\";\nimport { area } from \"./svc\";\n\n// The width.\nexport const WIDTH = 2;\n\nfunction build(): number {\n  return 1;\n}\n\nit(\"squares\", () => {\n  expect(area(WIDTH)).toBe(4);\n});\n\nit(\"builds\", () => {\n  expect(build()).toBe(1);\n});\n";

#[test]
fn typescript_keeps_one_blank_line_where_two_runs_were_bridged() {
    let Some(dir) = project(
        "lang-typescript",
        lighthouse_test_support::lang_typescript(),
        &[
            (
                "package.json",
                "{\"name\":\"d\",\"version\":\"1.0.0\",\"type\":\"module\"}",
            ),
            ("svc.ts", TS_SOURCE),
            ("svc.test.ts", TS),
        ],
    ) else {
        return;
    };

    assert_eq!(fixed(&dir, "testing/file-layout", "svc.test.ts"), TS_FIXED);
}

const RUST: &str = "/// Doubles.\n#[inline]\npub fn double() -> i32 {\n    LIMIT * 2\n}\n\n\n/// The limit.\n#[allow(dead_code)]\nconst LIMIT: i32 = 3;\n\n/// The name.\nstatic NAME: &str = \"n\";\n\n/// Uses the name.\npub fn name() -> &'static str {\n    NAME\n}\n";
const RUST_FIXED: &str = "/// The limit.\n#[allow(dead_code)]\nconst LIMIT: i32 = 3;\n\n/// The name.\nstatic NAME: &str = \"n\";\n\n/// Doubles.\n#[inline]\npub fn double() -> i32 {\n    LIMIT * 2\n}\n\n/// Uses the name.\npub fn name() -> &'static str {\n    NAME\n}\n";

#[test]
fn rust_keeps_attributes_and_docs_and_the_usual_separation() {
    let dir = rust_project(RUST, "");

    assert_eq!(
        fixed(&dir, "design/declaration-groups", "src/lib.rs"),
        RUST_FIXED
    );
}

const GO: &str = "package demo\n\n// helper doubles.\nfunc helper() int {\n\treturn limit * 2\n}\n\n// Service runs.\ntype Service struct{}\n\n// Run runs.\nfunc (s *Service) Run() int {\n\treturn helper()\n}\n\nconst (\n\t// limit.\n\tlimit = 1\n\tother = 2\n)\n\nvar ready = true\n";
const GO_FIXED: &str = "package demo\n\n// Service runs.\ntype Service struct{}\n\nconst (\n\t// limit.\n\tlimit = 1\n\tother = 2\n)\n\nvar ready = true\n\n// Run runs.\nfunc (s *Service) Run() int {\n\treturn helper()\n}\n\n// helper doubles.\nfunc helper() int {\n\treturn limit * 2\n}\n";

#[test]
fn go_moves_a_const_group_and_a_var_with_their_comments_and_one_blank_line() {
    let Some(dir) = go_project(&[("a.go", GO)], "") else {
        return;
    };

    assert_eq!(fixed(&dir, "design/declaration-groups", "a.go"), GO_FIXED);
}

const GO_NO_END: &str = "package demo\n\nimport \"testing\"\n\nfunc TestRows(t *testing.T) {\n\tif build().n != limit {\n\t\tt.Fatal(\"rows\")\n\t}\n}\n\nvar limit = base + 1\n\nfunc build() row {\n\treturn row{n: base}\n}\n\nconst base = 2\n\ntype row struct{ n int }";
const GO_NO_END_FIXED: &str = "package demo\n\nimport \"testing\"\n\ntype row struct{ n int }\n\nconst base = 2\n\nvar limit = base + 1\n\nfunc build() row {\n\treturn row{n: base}\n}\n\nfunc TestRows(t *testing.T) {\n\tif build().n != limit {\n\t\tt.Fatal(\"rows\")\n\t}\n}";

#[test]
fn go_fixes_a_test_file_without_a_final_line_end_in_one_reorder() {
    let Some(dir) = go_project(&[("a_test.go", GO_NO_END)], "") else {
        return;
    };

    assert_eq!(
        fixed(&dir, "testing/file-layout", "a_test.go"),
        GO_NO_END_FIXED
    );
}

const PYTHON_NO_END: &str = "def test_make():\n    assert make() == 4\n\n\nLIMIT = 4\n\n\ndef register(limit):\n    def wrap(function):\n        return function\n\n    return wrap\n\n\n@register(LIMIT)\ndef make(size=LIMIT):\n    return size\n\n\nclass Base:\n    pass\n\n\nclass Row(Base):\n    pass";

#[test]
fn python_fixes_a_test_file_without_a_final_line_end_in_one_reorder() {
    let Some(dir) = python(&[("test_rows.py", PYTHON_NO_END)]) else {
        return;
    };

    let fixed = fixed(&dir, "testing/file-layout", "test_rows.py");

    assert!(!fixed.ends_with('\n'), "{fixed:?}");
    assert!(
        fixed.find("class Row").unwrap() < fixed.find("def test_make").unwrap(),
        "{fixed}"
    );
    assert!(
        fixed.find("def make").unwrap() < fixed.find("def test_make").unwrap(),
        "{fixed}"
    );
}
