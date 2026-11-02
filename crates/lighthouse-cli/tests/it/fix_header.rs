//! A fix that moves declarations keeps every symbol of the file where it is:
//! no declaration leaves its class or file scope, no test is lost, and none is
//! put above the imports. Python, whose scopes are indentation, TypeScript and
//! Rust, over the real language plugins.

use predicates::prelude::*;
use tempfile::TempDir;

use crate::fix::{lighthouse, read, rust_project, write};

/// A project of `files` with the testing rules, in the language of `plugin`
/// (`lang_python()`, say) as the plugin id `id`; `None` after a skip message
/// when the language has no runtime here.
fn project(
    id: &str,
    plugin: Option<std::path::PathBuf>,
    files: &[(&str, &str)],
) -> Option<TempDir> {
    let plugin = plugin?;
    let dir = tempfile::tempdir().unwrap();
    lighthouse_test_support::installed(&dir.path().join(".home"), &plugin);
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"{id}\"]\nextends = [\"design/recommended\", \"testing/recommended\"]\n[rules]\n\"core/formatted\" = \"off\"\n"
        )),
    );
    for (name, text) in files {
        write(dir.path(), name, text);
    }
    Some(dir)
}

fn python(files: &[(&str, &str)]) -> Option<TempDir> {
    project("lang-python", lighthouse_test_support::lang_python(), files)
}

const SHAPES: &str =
    "def area(side):\n    return side * side\n\n\ndef side(shape):\n    return shape.side\n";

/// `test_area` tests `area`, declared before `side`, and so belongs above
/// `test_side`, a method of a class: no move can put one next to the other.
const MIXED: &str = "import unittest\n\nfrom shapes import area, side\n\n\nclass ShapeTest(unittest.TestCase):\n    def test_width(self):\n        self.assertEqual(area(1), 1)\n        return None\n\n    def test_side(self):\n        self.assertEqual(side(self), 3)\n\n\ndef test_area():\n    assert area(3) == 9\n";

/// A fixture function after the tests of a class: it belongs above them, and
/// the first of them is a method.
const FIXTURE_AFTER_CLASS: &str = "import unittest\n\nfrom shapes import area\n\n\nclass ShapeTest(unittest.TestCase):\n    def test_area(self):\n        self.assertEqual(area(build()), 9)\n\n\ndef build():\n    return 3\n";

/// Fixtures after the tests of a pytest module, a unittest class and a late
/// import that a fixture reads.
const PYTEST: &str = "import pytest\n\nfrom shapes import area\n\n\ndef test_area():\n    assert area(build()) == 9\n\n\ndef test_again():\n    assert area(WIDTH) == 9\n\n\n@pytest.fixture\ndef build():\n    return 3\n\n\nWIDTH = 3\n";

const LATE_IMPORT: &str =
    "def test_area():\n    assert WIDTH == 3\n\nimport os\n\nWIDTH = len(os.sep) + 2\n";

/// The `def test_` lines of a Python file.
fn tests(text: &str) -> usize {
    text.lines()
        .filter(|l| l.trim_start().starts_with("def test_"))
        .count()
}

#[test]
fn a_test_method_is_not_moved_into_the_body_of_a_function() {
    let Some(dir) = python(&[("shapes.py", SHAPES), ("test_shapes.py", MIXED)]) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "testing/file-layout"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("the fix changes nothing"));

    assert_eq!(read(&dir, "test_shapes.py"), MIXED, "no test is displaced");
}

#[test]
fn a_function_is_not_moved_into_a_class() {
    let Some(dir) = python(&[
        ("shapes.py", SHAPES),
        ("test_shapes.py", FIXTURE_AFTER_CLASS),
    ]) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "testing/file-layout"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("rolled back").not())
        .stderr(predicate::str::contains("not in the same block"));

    assert_eq!(read(&dir, "test_shapes.py"), FIXTURE_AFTER_CLASS);
}

#[test]
fn fixtures_move_above_the_tests_below_the_imports_and_the_second_fix_is_a_no_op() {
    let Some(dir) = python(&[("shapes.py", SHAPES), ("test_shapes.py", PYTEST)]) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "testing/file-layout"])
        .assert()
        .stderr(predicate::str::contains("rolled back").not());

    let fixed = read(&dir, "test_shapes.py");
    assert!(
        fixed.starts_with("import pytest\n\nfrom shapes import area\n\n"),
        "{fixed}"
    );
    assert!(
        fixed.find("def build").unwrap() < fixed.find("def test_area").unwrap(),
        "{fixed}"
    );
    assert_eq!(tests(&fixed), tests(PYTEST), "every test is still there");

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "testing/file-layout"])
        .assert()
        .success();
    assert_eq!(
        read(&dir, "test_shapes.py"),
        fixed,
        "the second fix is a no-op"
    );
}

#[test]
fn a_constant_is_not_moved_above_a_late_import() {
    let Some(dir) = python(&[("test_width.py", LATE_IMPORT)]) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "testing/file-layout"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("above the imports"));

    assert_eq!(read(&dir, "test_width.py"), LATE_IMPORT);
}

const TS_SOURCE: &str = "export function area(n: number): number {\n  return n * n;\n}\n";

/// A test, an import the fixture after it reads, and the fixture.
const TS_LATE_IMPORT: &str = "import { it, expect } from \"vitest\";\nimport { area } from \"./svc\";\n\nit(\"squares\", () => {\n  expect(area(WIDTH)).toBe(4);\n});\n\nimport { two } from \"./two\";\n\nexport const WIDTH = two;\n";

#[test]
fn a_typescript_fixture_is_not_moved_above_an_import() {
    let Some(dir) = project(
        "lang-typescript",
        lighthouse_test_support::lang_typescript(),
        &[
            (
                "package.json",
                "{\"name\":\"d\",\"version\":\"1.0.0\",\"type\":\"module\"}",
            ),
            ("svc.ts", TS_SOURCE),
            ("two.ts", "export const two = 2;\n"),
            ("svc.test.ts", TS_LATE_IMPORT),
        ],
    ) else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "testing/file-layout"])
        .assert()
        .stderr(predicate::str::contains("above the imports"));

    assert_eq!(read(&dir, "svc.test.ts"), TS_LATE_IMPORT);
}

const RUST_LATE_USE: &str = "/// A.\npub fn a() -> i32 {\n    1\n}\n\nuse std::cmp::max;\n\n/// B.\npub fn b() -> i32 {\n    max(a(), LIMIT)\n}\n\n/// The limit.\nconst LIMIT: i32 = 3;\n";

#[test]
fn a_rust_constant_is_not_moved_above_a_use() {
    let dir = rust_project(RUST_LATE_USE, "");

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "design/declaration-groups"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("above the imports"));

    assert_eq!(read(&dir, "src/lib.rs"), RUST_LATE_USE);
}
