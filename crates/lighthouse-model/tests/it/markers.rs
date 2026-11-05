//! The markers that make a file generated.

use std::{path::Path, sync::Arc};

use lighthouse_model::{
    Position, Span,
    ucm::{Comment, File, Fragment, Markers},
};

fn patterns() -> Vec<String> {
    vec!["DO NOT EDIT".to_owned()]
}

fn commented(text: &str) -> Fragment {
    let at = Position { line: 1, col: 1 };
    Fragment {
        files: vec![File {
            path: "a.go".into(),
            lang: "go".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        comments: vec![Comment {
            file: "a.go".into(),
            span: Span { start: at, end: at },
            text: text.to_owned(),
            attached_to: None,
        }],
        ..Fragment::default()
    }
}

#[test]
fn markers_new() {
    assert!(Markers::new(patterns()).is_ok());
    assert!(Markers::new(vec!["(".to_owned()]).is_err());
}

#[test]
fn markers_patterns() {
    assert_eq!(Markers::new(patterns()).unwrap().patterns(), patterns());
}

#[test]
fn markers_shared() {
    let (one, two) = (
        Markers::shared(&patterns()).unwrap(),
        Markers::shared(&patterns()).unwrap(),
    );

    assert!(Arc::ptr_eq(&one, &two));
    assert!(Markers::shared(&["(".to_owned()]).is_err());
}

#[test]
fn markers_mark() {
    let markers = Markers::new(patterns()).unwrap();
    let (mut marked, mut other) = (commented("// DO NOT EDIT"), commented("// hand written"));

    markers.mark(&mut marked);
    markers.mark(&mut other);

    assert!(marked.files[0].generated);
    assert!(!other.files[0].generated);
}

#[test]
fn key() {
    assert_eq!(lighthouse_model::ucm::key(Path::new("a/b.go")), "a/b.go");
}
