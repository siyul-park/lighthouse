//! The programs a decision binds per language: `core/format/formatted` gives
//! one check and one fix for Go and another for Rust.

use lighthouse_spec::{
    CheckKind, CommandCheck, CommandSpec, Decision, FixKind, Point, Program, Programs, Stream,
};

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
    assert_eq!(names, ["gofmt", "ruff", "rustfmt", "prettier"]);
}

fn parsed(table: &str) -> Program {
    let mut program: Program = serde_norway::from_str(table).unwrap();
    program.command = vec!["go".to_owned()];
    program
}

#[test]
fn stream_stderr_is_refused_unless_a_check_reads_lines() {
    let status = parsed("output: status\nstream: stderr\n");
    let lines = parsed("output: lines\nstream: stderr\n");
    let fix = parsed("output: files\nstream: stderr\n");

    assert!(status.problem(Point::Check).unwrap().contains("stream"));
    assert!(fix.problem(Point::Fix).unwrap().contains("stream"));
    assert!(lines.problem(Point::Verify).unwrap().contains("stream"));
    assert_eq!(lines.problem(Point::Check), None);
}

#[test]
fn the_default_stream_is_stdout_and_is_not_written() {
    let program = parsed("output: lines\n");

    let written = serde_json::to_value(&program).unwrap();

    assert_eq!(program.stream, Stream::Stdout);
    assert!(written.get("stream").is_none(), "{written}");
    assert_eq!(
        serde_json::to_value(parsed("stream: stderr\n")).unwrap()["stream"],
        "stderr"
    );
}
