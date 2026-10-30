use super::*;

fn commands(line: &str) -> Vec<(String, Vec<String>)> {
    segments(line)
        .into_iter()
        .map(|s| (s.word, s.args))
        .collect()
}

fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

#[test]
fn segments_cut_a_pipeline_at_each_command() {
    assert_eq!(
        commands("cat a.go | rg Parse"),
        [
            ("cat".to_owned(), words(&["a.go"])),
            ("rg".to_owned(), words(&["Parse"])),
        ]
    );
}

#[test]
fn segments_cut_at_and_or_semicolon_and_line_break() {
    let line = "cd pkg && ls || echo no; pwd\nid";
    let found: Vec<String> = segments(line).into_iter().map(|s| s.word).collect();

    assert_eq!(found, ["cd", "ls", "echo", "pwd", "id"]);
}

#[test]
fn segments_keep_what_is_quoted_in_one_word() {
    assert_eq!(
        commands(r#"rg -n 'a | b' "dir with space" x\ y"#),
        [(
            "rg".to_owned(),
            words(&["-n", "a | b", "dir with space", "x y"])
        )]
    );
}

#[test]
fn segments_read_the_script_a_shell_is_given() {
    assert_eq!(
        commands(r#"/bin/zsh -lc "sed -n '1,120p' pkg/a.go""#),
        [("sed".to_owned(), words(&["-n", "1,120p", "pkg/a.go"]))]
    );
    assert_eq!(
        commands("bash -c 'cat a.go | head -3'")
            .into_iter()
            .map(|c| c.0)
            .collect::<Vec<_>>(),
        ["cat", "head"]
    );
}

#[test]
fn segments_skip_assignments_wrappers_and_the_directory_of_a_program() {
    assert_eq!(
        commands("FOO=1 env /usr/bin/cat a.go"),
        [("cat".to_owned(), words(&["a.go"]))]
    );
}

#[test]
fn segments_leave_out_redirections() {
    assert_eq!(
        commands("go test ./... > out.txt 2>&1"),
        [("go".to_owned(), words(&["test", "./..."]))]
    );
    assert_eq!(
        commands("cat a.go 2> /dev/null"),
        [("cat".to_owned(), words(&["a.go"]))]
    );
}

#[test]
fn segments_say_the_command_as_it_was_written() {
    let found = segments("cd x &&  go test -run cat ./...  ");

    assert_eq!(found[1].text, "go test -run cat ./...");
}

#[test]
fn segments_stop_looking_into_scripts_at_a_depth() {
    let line = "sh -c \"sh -c \\\"sh -c \\\\\\\"sh -c 'cat a'\\\\\\\"\\\"\"";

    assert_eq!(commands(line).len(), 1);
}

#[test]
fn operands_are_the_arguments_that_are_not_options() {
    let found = &segments("rg -n Parse pkg other")[0];

    assert_eq!(found.operands(), ["Parse", "pkg", "other"]);
}
