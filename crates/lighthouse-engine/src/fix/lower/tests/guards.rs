//! What lowering refuses to touch: split characters, files outside the project, generated files, symlinks.

use super::*;

#[test]
fn a_korean_banner_and_a_multibyte_end_do_not_split_a_character() {
    let text = "// 도우미 함수들\nfn a() {} // 끝\n\nfn b() { \"한\" }\n";
    let mut case = case(text, &[("a", 2, 2), ("b", 4, 4)], vec![]);
    // `b`'s extent ends right after the multibyte `}` neighbour: a span
    // whose last byte is inside a character must not panic.
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[1].extent = Some(span((4, 1), (4, 17)));
    case.project = Project::merge([fragment]);

    let moved = case.run(&[after("a", "b")]).unwrap();

    assert_eq!(
        moved,
        "// 도우미 함수들\nfn b() { \"한\" }\n\nfn a() {} // 끝\n"
    );
    let deleted = case.run(&[EditOp::DeleteRange {
        file: FILE.into(),
        span: span((1, 1), (1, 23)),
    }]);
    assert_eq!(deleted.unwrap(), "fn a() {} // 끝\n\nfn b() { \"한\" }\n");
}

#[test]
fn an_extent_that_ends_inside_a_character_is_refused_not_a_panic() {
    let text = "fn a() { \"한\" }\n\nfn b() {}\n";
    let mut case = case(text, &[("a", 1, 1), ("b", 3, 3)], vec![]);
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[0].extent = Some(span((1, 1), (1, 12)));
    case.project = Project::merge([fragment]);

    let why = case.run(&[after("a", "b")]).unwrap_err();

    assert!(why.contains("outside its file"), "{why}");
}

fn contained(file: &str) -> Result<String, String> {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3)], vec![]);
    case.run(&[EditOp::Replace {
        file: file.into(),
        span: span((1, 1), (1, 1)),
        text: "x".to_owned(),
    }])
}

#[test]
fn edits_stay_inside_relative_normalized_project_files() {
    assert!(
        contained("../a.src")
            .unwrap_err()
            .contains("not a relative, normalized")
    );
    assert!(
        contained("/etc/passwd")
            .unwrap_err()
            .contains("not a relative, normalized")
    );
    assert!(
        contained("./a.src")
            .unwrap_err()
            .contains("not a relative, normalized")
    );
    assert!(
        contained("other.src")
            .unwrap_err()
            .contains("not a file of the project")
    );
}

#[test]
fn generated_files_are_not_edited() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), "x\n").unwrap();
    let project = Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "src".to_owned(),
            hash: String::new(),
            generated: true,
            test: false,
            facts: Default::default(),
        }],
        ..Fragment::default()
    }]);
    let overlays = Overlays::new();
    let sources = Sources::new(dir.path(), &project, &overlays);

    assert!(
        sources
            .text(Path::new(FILE))
            .unwrap_err()
            .contains("generated")
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_file_or_directory_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("x.src"), "x\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("sub")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("x.src"), dir.path().join("link.src")).unwrap();
    let file = |path: &str| File {
        path: path.into(),
        lang: "src".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
        facts: Default::default(),
    };
    let project = Project::merge([Fragment {
        files: vec![file("sub/x.src"), file("link.src")],
        ..Fragment::default()
    }]);
    let overlays = Overlays::new();
    let sources = Sources::new(dir.path(), &project, &overlays);

    for path in ["sub/x.src", "link.src"] {
        let why = sources.text(Path::new(path)).unwrap_err();
        assert!(why.contains("symlink"), "{path}: {why}");
    }
}
