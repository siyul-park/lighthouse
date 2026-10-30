use super::*;

fn roots() -> Roots {
    Roots {
        dir: PathBuf::from("/work/scratch"),
        forms: vec![PathBuf::from("/work/scratch")],
    }
}

#[test]
fn locate_makes_a_path_below_the_root_relative() {
    assert_eq!(
        roots().locate("/work/scratch/pkg/a.go").as_deref(),
        Some("pkg/a.go")
    );
    assert_eq!(roots().locate("/work/scratch").as_deref(), Some("."));
}

#[test]
fn locate_folds_dots_double_slashes_and_parents() {
    let roots = roots();

    assert_eq!(roots.locate("./pkg//a.go").as_deref(), Some("pkg/a.go"));
    assert_eq!(roots.locate("pkg/../b.go").as_deref(), Some("b.go"));
    assert_eq!(
        roots.locate("/work/scratch//pkg/./x/../a.go").as_deref(),
        Some("pkg/a.go")
    );
    assert_eq!(roots.locate("pkg/..").as_deref(), Some("."));
}

#[test]
fn locate_refuses_what_climbs_out_of_the_root() {
    let roots = roots();

    assert_eq!(roots.locate("../a.go"), None);
    assert_eq!(roots.locate("pkg/../../a.go"), None);
    assert_eq!(roots.locate("/work/scratch/../other/a.go"), None);
    assert_eq!(roots.locate("/etc/passwd"), None);
}

#[test]
fn locate_does_not_mistake_a_sibling_with_the_same_prefix_for_the_root() {
    assert_eq!(roots().locate("/work/scratch2/a.go"), None);
}

#[test]
fn locate_knows_every_root_the_directory_has() {
    let roots = Roots {
        dir: PathBuf::from("/var/folders/x/scratch"),
        forms: vec![
            PathBuf::from("/var/folders/x/scratch"),
            PathBuf::from("/private/var/folders/x/scratch"),
        ],
    };

    assert_eq!(
        roots
            .locate("/private/var/folders/x/scratch/a.go")
            .as_deref(),
        Some("a.go")
    );
    assert_eq!(
        roots.locate("/var/folders/x/scratch/a.go").as_deref(),
        Some("a.go")
    );
}

#[test]
fn place_drops_a_path_outside_and_marks_the_event() {
    let event = |path: &str| ToolEvent {
        tool: Tool::Read,
        path: Some(path.to_owned()),
        outside: false,
        pattern: None,
        command: None,
    };

    let outside = roots().place(event("/Users/someone/.ssh/id_rsa"));
    let inside = roots().place(event("/work/scratch/a.go"));

    assert_eq!((outside.path, outside.outside), (None, true));
    assert_eq!(
        (inside.path.as_deref(), inside.outside),
        (Some("a.go"), false)
    );
}

#[test]
fn tail_cuts_at_a_character_boundary() {
    assert_eq!(tail("héllo", 3), "llo");
    assert_eq!(tail("héllo", 4), "llo");
    assert_eq!(tail("héllo", 5), "éllo");
    assert_eq!(tail("ab", 9), "ab");
}

#[test]
fn place_marks_a_pattern_that_is_a_path_outside_and_keeps_the_other_patterns() {
    let search = |pattern: &str| ToolEvent {
        tool: Tool::Search,
        path: None,
        outside: false,
        pattern: Some(pattern.to_owned()),
        command: None,
    };

    let outside = roots().place(search("/Users/someone/.ssh/id_rsa"));
    let inside = roots().place(search("/work/scratch/pkg/a.go"));
    let plain = roots().place(search("func /Parse/"));
    let regex = roots().place(search("/Parse.*/"));

    assert_eq!((outside.pattern, outside.outside), (None, true));
    assert_eq!(
        (inside.pattern.as_deref(), inside.outside),
        (Some("pkg/a.go"), false)
    );
    assert_eq!(plain.pattern.as_deref(), Some("func /Parse/"));
    assert_eq!(regex.pattern, None, "an absolute word is taken for a path");
}

#[cfg(unix)]
mod links {
    use std::os::unix::fs::symlink;

    use super::*;

    fn read(path: &str) -> ToolEvent {
        ToolEvent {
            tool: Tool::Read,
            path: Some(path.to_owned()),
            outside: false,
            pattern: None,
            command: None,
        }
    }

    /// A directory with `real/a.go`, `link` to `real`, `file-link` to
    /// `real/a.go`.
    fn scratch() -> (tempfile::TempDir, Roots) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("real")).unwrap();
        fs::write(dir.path().join("real/a.go"), "x").unwrap();
        symlink("real", dir.path().join("link")).unwrap();
        symlink("real/a.go", dir.path().join("file-link")).unwrap();
        let roots = Roots::of(dir.path());
        (dir, roots)
    }

    #[test]
    fn a_path_that_crosses_a_link_is_outside_and_has_no_name() {
        let (_dir, roots) = scratch();

        for path in [
            "link/a.go",
            "file-link",
            "link/../real/a.go",
            "real/../link",
        ] {
            let placed = roots.place(read(path));
            assert_eq!((placed.path, placed.outside), (None, true), "{path}");
        }
        let plain = roots.place(read("real/a.go"));
        assert_eq!(plain.path.as_deref(), Some("real/a.go"));
    }

    #[test]
    fn ground_does_not_follow_a_link_to_a_file() {
        let (_dir, roots) = scratch();
        let linked = ToolEvent {
            path: Some("file-link".to_owned()),
            ..read("")
        };

        let kept = roots.ground(vec![linked]);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].path, None);
    }
}

fn call(tool: Tool, paths: &[&str]) -> Vec<ToolEvent> {
    paths
        .iter()
        .map(|path| ToolEvent {
            tool,
            path: Some((*path).to_owned()),
            outside: false,
            pattern: None,
            command: None,
        })
        .collect()
}

fn kept(events: &[ToolEvent]) -> Vec<Option<&str>> {
    events.iter().map(|e| e.path.as_deref()).collect()
}

#[test]
fn ground_keeps_a_file_without_a_dot_and_drops_the_operand_that_is_no_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Makefile"), "all:\n").unwrap();
    std::fs::write(dir.path().join("x.go"), "package x\n").unwrap();
    let roots = Roots::of(dir.path());

    let make = roots.ground(call(Tool::Read, &["Makefile"]));
    let sed = roots.ground(call(Tool::Read, &["s/a/b/", "x.go"]));

    assert_eq!(kept(&make), [Some("Makefile")]);
    assert_eq!(kept(&sed), [Some("x.go")]);
}

#[test]
fn ground_lets_a_search_name_a_directory_and_a_read_only_a_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("pkg")).unwrap();
    let roots = Roots::of(dir.path());

    let search = roots.ground(call(Tool::Search, &["pkg", "."]));
    let read = roots.ground(call(Tool::Read, &["pkg"]));

    assert_eq!(kept(&search), [Some("pkg"), Some(".")]);
    assert_eq!(kept(&read), [None], "a call whose paths name nothing stays");
}

#[test]
fn ground_leaves_alone_what_is_not_a_read_or_a_search() {
    let roots = Roots::of(tempfile::tempdir().unwrap().path());

    let edit = roots.ground(call(Tool::Edit, &["new.go"]));
    let list = roots.ground(call(Tool::List, &["*.go"]));

    assert_eq!(kept(&edit), [Some("new.go")]);
    assert_eq!(kept(&list), [Some("*.go")]);
}
