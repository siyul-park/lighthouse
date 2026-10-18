//! The safe reorder of `design/declaration-groups` over Go files with
//! parenthesized declarations, which it moves as one unit or declines to move.

use predicates::prelude::*;

use crate::fix::{go_project, lighthouse, read};

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
            "a grouped declaration mixes groups",
        ))
        .stderr(predicate::str::contains("split it"));

    assert_eq!(read(&dir, "mixed.go"), MIXED, "nothing is displaced");
    assert_eq!(read(&dir, "sub/b.go"), ORDERED);
}
