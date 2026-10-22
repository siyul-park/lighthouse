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

const LAYOUT: &str = "[rules]\n\"design/layout\" = \"warn\"\n";

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
        findings(&dir, "design/layout"),
        1,
        "Size is separated from ID by bound"
    );

    for decision in ["design/declaration-groups", "design/layout"] {
        lighthouse(dir.path())
            .args(["check", "--fix", "--decision", decision])
            .assert()
            .success()
            .stderr(predicate::str::contains("rolled back").not())
            .stderr(predicate::str::contains("not fixed").not());
    }
    let fixed = read(&dir, "decls.go");
    assert_eq!(findings(&dir, "design/declaration-groups"), 0);
    assert_eq!(findings(&dir, "design/layout"), 0);

    for decision in ["design/layout", "design/declaration-groups"] {
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
