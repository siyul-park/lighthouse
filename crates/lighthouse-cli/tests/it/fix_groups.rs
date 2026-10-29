//! The safe reorder of `design/declaration-groups` over Go files with
//! parenthesized declarations, which it moves as one unit or declines to move.

use predicates::prelude::*;

use crate::fix::{go_project, lighthouse, read, rust_project};

const ISLAND: &str = "package demo\n\nconst (\n\tModeA = \"a\"\n\tModeB = \"b\"\n)\n\nfunc Public() int { return 1 }\n\ntype One struct{}\n\ntype Two struct{}\n\ntype Three struct{}\n";
const ISLAND_ORDERED: &str = "package demo\n\ntype One struct{}\n\ntype Two struct{}\n\ntype Three struct{}\n\nconst (\n\tModeA = \"a\"\n\tModeB = \"b\"\n)\n\nfunc Public() int { return 1 }\n";

const MIXED: &str =
    "package demo\n\nconst (\n\tmodeB = \"b\"\n\tModeA = \"a\"\n)\n\ntype One struct{}\n";

const MISORDERED: &str = "package sub\n\nfunc helper() int { return 1 }\n\ntype Service struct{}\n\nfunc (s *Service) Run() int { return helper() }\n";
const ORDERED: &str = "package sub\n\ntype Service struct{}\n\nfunc (s *Service) Run() int { return helper() }\n\nfunc helper() int { return 1 }\n";

#[test]
fn a_const_group_moves_as_one_unit_and_the_file_converges() {
    let Some(dir) = go_project(&[("grouped.go", ISLAND)], "") else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "design/declaration-groups"])
        .assert()
        .success()
        .stderr(predicate::str::contains("rolled back").not())
        .stderr(predicate::str::contains(
            "fixed design/declaration-groups [safe] grouped.go",
        ));

    assert_eq!(read(&dir, "grouped.go"), ISLAND_ORDERED);
    lighthouse(dir.path())
        .args(["check", "--decision", "design/declaration-groups"])
        .assert()
        .success()
        .stdout(predicate::str::contains("declaration-groups").not());
}

#[test]
fn a_group_that_mixes_groups_is_declined_with_its_reason_and_other_files_still_apply() {
    let Some(dir) = go_project(&[("mixed.go", MIXED), ("sub/b.go", MISORDERED)], "") else {
        return;
    };

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "design/declaration-groups"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("rolled back").not())
        .stderr(predicate::str::contains(
            "`modeB` and `ModeA` of one grouped declaration are ordered against their file order",
        ))
        .stderr(predicate::str::contains("split it"));

    assert_eq!(read(&dir, "mixed.go"), MIXED, "nothing is displaced");
    assert_eq!(read(&dir, "sub/b.go"), ORDERED);
}

/// The shape of cel's `common/decls`: a group of private consts, a type whose
/// private method the group order moves away from its public ones, a free
/// function between the methods, public types after them and a constructor.
const DECLS: &str = "package demo

const (
	limitLow  = 1
	limitHigh = 9
)

// Overload is a callable.
type Overload struct{ n int }

// ID names the overload.
func (o *Overload) ID() int { return o.n }

func bound(n int) int { return min(max(n, limitLow), limitHigh) }

// Size bounds the overload.
func (o *Overload) Size() int { return o.n }

func (o *Overload) bounded() int { return o.n + 1 }

// Function groups overloads.
type Function struct{ overloads []*Overload }

// NewFunction returns a function without overloads.
func NewFunction() *Function { return &Function{} }

// Count reports the overloads.
func (f *Function) Count() int { return len(f.overloads) }
";

const LAYOUT: &str = "[rules]\n\"design/top-down-order\" = \"warn\"\n";

/// How many findings of `decision` the project has.
fn findings(dir: &tempfile::TempDir, decision: &str) -> usize {
    let found = lighthouse(dir.path())
        .args(["check", "--decision", decision])
        .output()
        .unwrap();
    String::from_utf8(found.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.contains(&format!(" {decision}: ")))
        .count()
}

#[test]
fn the_group_order_and_the_layout_agree_on_a_real_file_and_reach_a_fixpoint() {
    let Some(dir) = go_project(&[("decls.go", DECLS)], LAYOUT) else {
        return;
    };
    assert_eq!(
        findings(&dir, "design/vertical-distance"),
        1,
        "Size is separated from ID by bound"
    );

    for decision in ["design/declaration-groups", "design/vertical-distance"] {
        lighthouse(dir.path())
            .args(["check", "--fix", "--decision", decision])
            .assert()
            .success()
            .stderr(predicate::str::contains("rolled back").not())
            .stderr(predicate::str::contains("not fixed").not());
    }
    let fixed = read(&dir, "decls.go");
    assert_eq!(findings(&dir, "design/declaration-groups"), 0);
    assert_eq!(findings(&dir, "design/vertical-distance"), 0);

    for decision in ["design/vertical-distance", "design/declaration-groups"] {
        lighthouse(dir.path())
            .args(["check", "--fix", "--decision", decision])
            .assert()
            .success();
    }
    assert_eq!(
        read(&dir, "decls.go"),
        fixed,
        "the fixes do not undo each other"
    );
    let owners: Vec<&str> = fixed
        .lines()
        .filter(|line| line.starts_with("func "))
        .collect();
    assert_eq!(
        owners,
        [
            "func (o *Overload) ID() int { return o.n }",
            "func (o *Overload) Size() int { return o.n }",
            "func (o *Overload) bounded() int { return o.n + 1 }",
            "func NewFunction() *Function { return &Function{} }",
            "func (f *Function) Count() int { return len(f.overloads) }",
            "func bound(n int) int { return min(max(n, limitLow), limitHigh) }",
        ],
        "types first, then one block per type in declaration order, then the free functions"
    );
}

#[test]
fn a_rust_file_with_parentheses_is_reordered_as_before() {
    let source = "pub fn run() -> u8 {\n    helper((1, 2))\n}\n\npub struct Store(u8, (u8, u8));\n\nfn helper(pair: (u8, u8)) -> u8 {\n    pair.0\n}\n";
    let dir = rust_project(source, "");

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "design/declaration-groups"])
        .assert()
        .success()
        .stderr(predicate::str::contains("rolled back").not());

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub struct Store(u8, (u8, u8));\n\npub fn run() -> u8 {\n    helper((1, 2))\n}\n\nfn helper(pair: (u8, u8)) -> u8 {\n    pair.0\n}\n"
    );
}

const COMMENTED: &str = "// helper comment\n// second line\nfn helper() -> u8 { 1 }\n\n// Run comment\npub fn run() -> u8 { helper() }\n\n// Store comment\npub struct Store;\n";
const COMMENTED_ORDERED: &str = "// Store comment\npub struct Store;\n\n// Run comment\npub fn run() -> u8 { helper() }\n\n// helper comment\n// second line\nfn helper() -> u8 { 1 }\n";

#[test]
fn a_rust_reorder_moves_the_plain_comments_with_their_items() {
    let dir = rust_project(COMMENTED, "");

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "design/declaration-groups"])
        .assert()
        .success()
        .stderr(predicate::str::contains("rolled back").not());

    assert_eq!(read(&dir, "src/lib.rs"), COMMENTED_ORDERED);
}

#[test]
fn the_group_order_and_the_layout_agree_on_a_rust_file_with_comments() {
    let dir = rust_project(COMMENTED, "[rules]\n\"design/top-down-order\" = \"warn\"\n");

    for decision in ["design/declaration-groups", "design/top-down-order"] {
        lighthouse(dir.path())
            .args(["check", "--fix", "--decision", decision])
            .assert()
            .success();
    }
    assert_eq!(read(&dir, "src/lib.rs"), COMMENTED_ORDERED);
    assert_eq!(findings(&dir, "design/declaration-groups"), 0);
    assert_eq!(findings(&dir, "design/top-down-order"), 0);
}

/// 500 types, then their methods in the reverse order of the types: every
/// block but one is displaced, the shape that made the old expression crawl.
fn reversed_blocks(types: usize) -> String {
    let mut text = String::from("package big\n\n");
    for i in 0..types {
        text.push_str(&format!("type T{i} struct{{}}\n\n"));
    }
    for i in (0..types).rev() {
        text.push_str(&format!("func (t *T{i}) Run() int {{ return {i} }}\n\n"));
    }
    text
}

#[test]
fn a_file_of_a_thousand_reversed_declarations_is_ordered_in_bounded_time() {
    let source = reversed_blocks(500);
    let Some(dir) = go_project(&[("big.go", &source)], "") else {
        return;
    };
    let started = std::time::Instant::now();

    lighthouse(dir.path())
        .args(["check", "--fix", "--decision", "design/declaration-groups"])
        .assert()
        .success();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(90),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(findings(&dir, "design/declaration-groups"), 0);
}

const CALLER_FIRST: &str =
    "package demo\n\nfunc Run() int { return helper() }\n\nfunc helper() int { return 1 }\n";
const HELPER_FIRST: &str =
    "package demo\n\nfunc helper() int { return 1 }\n\nfunc Run() int { return helper() }\n";
const PRIVATE_FIRST: &str = "[rules]\n\"design/top-down-order\" = \"warn\"\n\"design/declaration-groups\" = { level = \"warn\", options = { groups = [\"private-function\", \"public-function\"] } }\n";

#[test]
fn the_layout_reads_the_group_order_the_project_set_for_the_group_decision() {
    let Some(plain) = go_project(&[("a.go", HELPER_FIRST)], LAYOUT) else {
        return;
    };
    assert_eq!(
        findings(&plain, "design/top-down-order"),
        1,
        "helper precedes its caller"
    );
    assert_eq!(findings(&plain, "design/declaration-groups"), 1);

    let Some(set) = go_project(&[("a.go", HELPER_FIRST)], PRIVATE_FIRST) else {
        return;
    };
    assert_eq!(
        findings(&set, "design/declaration-groups"),
        0,
        "the project puts private functions first"
    );
    assert_eq!(
        findings(&set, "design/top-down-order"),
        0,
        "the layout judges by the same order, set once"
    );
}

#[test]
fn an_order_set_for_the_group_decision_is_met_by_both_fixes_and_they_converge() {
    let Some(dir) = go_project(&[("a.go", CALLER_FIRST)], PRIVATE_FIRST) else {
        return;
    };
    assert_eq!(findings(&dir, "design/declaration-groups"), 1);
    assert_eq!(
        findings(&dir, "design/top-down-order"),
        0,
        "the order wants the helper first"
    );

    for decision in [
        "design/declaration-groups",
        "design/top-down-order",
        "design/declaration-groups",
    ] {
        lighthouse(dir.path())
            .args(["check", "--fix", "--decision", decision])
            .assert()
            .success()
            .stderr(predicate::str::contains("rolled back").not());
    }
    assert_eq!(read(&dir, "a.go"), HELPER_FIRST);
    assert_eq!(findings(&dir, "design/declaration-groups"), 0);
    assert_eq!(findings(&dir, "design/top-down-order"), 0);
}

#[test]
fn an_order_set_on_the_layout_itself_is_refused_it_is_read_from_the_group_decision() {
    let Some(dir) = go_project(
        &[("a.go", HELPER_FIRST)],
        "[rules]\n\"design/top-down-order\" = { level = \"warn\", options = { groups = [\"public-function\"] } }\n",
    ) else {
        return;
    };
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "read from `design/declaration-groups`",
        ));
}

const OTHER_HELPER_FIRST: &str =
    "package demo\n\nfunc other() int { return 1 }\n\nfunc Other() int { return other() }\n";
const PRIVATE_FIRST_FOR_B: &str = "[rules]\n\"design/top-down-order\" = \"warn\"\n\"design/declaration-groups\" = \"warn\"\n[[overrides]]\nfiles = [\"b.go\"]\nrules = { \"design/declaration-groups\" = { level = \"warn\", options = { groups = [\"private-function\", \"public-function\"] } } }\n";

/// How many findings of `decision` the project has in `file`.
fn findings_in(dir: &tempfile::TempDir, decision: &str, file: &str) -> usize {
    let found = lighthouse(dir.path())
        .args(["check", "--decision", decision, file])
        .output()
        .unwrap();
    String::from_utf8(found.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.contains(&format!(" {decision}: ")))
        .count()
}

#[test]
fn an_override_of_the_group_order_for_a_file_changes_the_layout_at_that_file_only() {
    let Some(dir) = go_project(
        &[("a.go", HELPER_FIRST), ("b.go", OTHER_HELPER_FIRST)],
        PRIVATE_FIRST_FOR_B,
    ) else {
        return;
    };
    assert_eq!(findings_in(&dir, "design/declaration-groups", "a.go"), 1);
    assert_eq!(findings_in(&dir, "design/declaration-groups", "b.go"), 0);
    assert_eq!(
        findings_in(&dir, "design/top-down-order", "a.go"),
        1,
        "the default order puts the caller first"
    );
    assert_eq!(
        findings_in(&dir, "design/top-down-order", "b.go"),
        0,
        "the layout judges b.go by the order the override sets for it"
    );

    for decision in [
        "design/declaration-groups",
        "design/top-down-order",
        "design/declaration-groups",
    ] {
        lighthouse(dir.path())
            .args(["check", "--fix", "--decision", decision])
            .assert()
            .success()
            .stderr(predicate::str::contains("rolled back").not());
    }
    assert_eq!(read(&dir, "a.go"), CALLER_FIRST);
    assert_eq!(read(&dir, "b.go"), OTHER_HELPER_FIRST);
    assert_eq!(findings(&dir, "design/declaration-groups"), 0);
    assert_eq!(findings(&dir, "design/top-down-order"), 0);
}
