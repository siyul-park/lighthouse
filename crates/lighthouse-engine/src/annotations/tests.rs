use super::*;

fn slice(text: &str, span: Span) -> &str {
    &text[(span.start.col as usize - 1)..(span.end.col as usize - 1)]
}

#[test]
fn a_rule_claim_removes_only_its_id_from_a_multi_rule_directive() {
    let text = "// lighthouse-disable-next-line a/x, b/y -- reason";
    let comment = Comment {
        file: PathBuf::from("x.go"),
        span: Span {
            start: Position { line: 1, col: 1 },
            end: Position {
                line: 1,
                col: count(text.len()) + 1,
            },
        },
        text: text.to_owned(),
        attached_to: None,
    };
    let (index, directive) = annotation::directives(text).pop().unwrap();
    let at = Located {
        comment: &comment,
        index,
        line: 1,
        directive,
    };

    assert_eq!(slice(text, rule_span(&at, Some(0))), "a/x, ");
    assert_eq!(slice(text, rule_span(&at, Some(1))), ", b/y");
}

#[test]
fn a_rule_that_did_not_finish_keeps_its_suppression_unsettled() {
    let file = PathBuf::from("x.go");
    let active = BTreeSet::from(["design/x".to_owned()]);
    let selected = active.clone();
    let unsettled = BTreeSet::from([("design/x".to_owned(), Some(file.clone()))]);
    let text = |_path: &Path| None;
    let gate = Gate {
        active: &active,
        selected: &selected,
        unsettled: &unsettled,
        text: &text,
    };

    assert!(!settled(&gate, "design/x", &file));
}

#[test]
fn non_ascii_code_before_a_comment_does_not_panic_when_finding_the_file_top() {
    let path = PathBuf::from("x.rs");
    let comment_text = "          // lighthouse-disable-next-line a/x -- reason";
    let project = Project::merge([lighthouse_model::ucm::Fragment {
        comments: vec![Comment {
            file: path.clone(),
            span: Span {
                start: Position { line: 2, col: 11 },
                end: Position {
                    line: 2,
                    col: count(comment_text.len()) + 1,
                },
            },
            text: comment_text.to_owned(),
            attached_to: None,
        }],
        ..Default::default()
    }]);
    let text =
        "let s = \"é\";\n          // lighthouse-disable-next-line a/x -- reason\nfn f() {}\n";
    assert_eq!(first_code_line(&project, &path, text), 1);
}

#[test]
fn a_multi_rule_enable_annotation_removes_only_the_unmatched_id() {
    let text = "// lighthouse-enable a/x, b/y";
    let comment = Comment {
        file: PathBuf::from("x.go"),
        span: Span {
            start: Position { line: 1, col: 1 },
            end: Position {
                line: 1,
                col: count(text.len()) + 1,
            },
        },
        text: text.to_owned(),
        attached_to: None,
    };
    let (index, directive) = annotation::directives(text).pop().unwrap();
    let at = Located {
        comment: &comment,
        index,
        line: 1,
        directive,
    };
    let finding = annotation(&at, "b/y", Some(false), Some(1));
    let span: Span = serde_json::from_value(finding["ruleExtent"].clone()).unwrap();

    assert_eq!(slice(text, span), ", b/y");
}

#[test]
fn a_single_rule_claim_removes_the_whole_directive() {
    let text = "// lighthouse-disable-next-line a/x -- reason";
    let comment = Comment {
        file: PathBuf::from("x.go"),
        span: Span {
            start: Position { line: 1, col: 1 },
            end: Position {
                line: 1,
                col: count(text.len()) + 1,
            },
        },
        text: text.to_owned(),
        attached_to: None,
    };
    let (index, directive) = annotation::directives(text).pop().unwrap();
    let at = Located {
        comment: &comment,
        index,
        line: 1,
        directive,
    };

    assert_eq!(slice(text, rule_span(&at, Some(0))), text);
}
