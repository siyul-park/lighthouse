//! The programs a decision binds per language: `core/format/formatted` gives
//! one check and one fix for Go and another for Rust.

use lighthouse_spec::{CheckKind, CommandCheck, CommandSpec, Decision, FixKind, Programs};

fn formatted() -> &'static Decision {
    lighthouse_test_support::bundled()
        .decision("core/formatted")
        .unwrap()
}

fn check_programs() -> Programs {
    let decision = formatted();
    let CheckKind::Command(check) = &decision.check.as_ref().unwrap().kind else {
        panic!("a command check is expected");
    };
    let check: &CommandCheck = check;
    check.programs(decision.spec())
}

fn fix_programs() -> Programs {
    let decision = formatted();
    let FixKind::Command(fix) = &decision.fix.as_ref().unwrap().kind else {
        panic!("a command fix is expected");
    };
    let fix: &CommandSpec = fix;
    fix.programs(decision.spec())
}

#[test]
fn a_check_takes_the_program_of_the_language() {
    let programs = check_programs();
    assert_eq!(programs.of("go").unwrap().command[0], "gofmt");
    assert_eq!(programs.of("rust").unwrap().command[0], "rustfmt");
}

#[test]
fn a_fix_takes_the_program_of_the_language() {
    let programs = fix_programs();
    assert_eq!(programs.of("go").unwrap().command[0], "gofmt");
    assert_eq!(programs.of("rust").unwrap().command[0], "rustfmt");
}

#[test]
fn a_language_without_a_program_has_none_when_the_decision_has_no_command() {
    let programs = check_programs();
    assert!(programs.of("yaml").is_none());
    assert!(programs.is_language("go"));
    assert!(!programs.is_language("yaml"));
}

#[test]
fn the_own_program_of_a_decision_with_languages_has_no_command() {
    assert!(check_programs().own().command.is_empty());
}

#[test]
fn all_lists_the_program_of_every_language() {
    let names: Vec<_> = check_programs()
        .all()
        .map(|program| program.command[0].clone())
        .collect();
    assert_eq!(names, ["gofmt", "rustfmt"]);
}
