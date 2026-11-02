//! The files of a project: what each source of "not ours" leaves out, once.

use std::{fs, path::Path};

use lighthouse_model::{Metadata, Resource};
use lighthouse_spec::{Attributes, Config, ProjectFiles, ProjectSpec, Projects};

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let at = dir.path().join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, text).unwrap();
    }
    dir
}

fn listed(dir: &Path, ignores: &[&str]) -> Vec<String> {
    let patterns: Vec<String> = ignores.iter().map(|s| (*s).to_owned()).collect();
    ProjectFiles::new(dir, &patterns)
        .unwrap()
        .list()
        .files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn each_source_of_not_ours_leaves_its_files_out_and_hidden_directories_stay() {
    let dir = tree(&[
        ("src/a.rs", ""),
        (".git/HEAD", ""),
        (".github/ci.yaml", ""),
        (".lighthouse/decisions/d.yaml", ""),
        (".lighthouse/cache/c.json", ""),
        (".lighthouse/lighthouse.db", ""),
        ("src/.lighthouse/cache/x", ""),
        (".gitignore", "out/\n*.tmp\n"),
        ("out/o.rs", ""),
        ("src/b.tmp", ""),
        ("pkg/.gitignore", "local.rs\n"),
        ("pkg/local.rs", ""),
        ("pkg/kept.rs", ""),
        ("other/local.rs", ""),
        ("fixtures/broken.rs", ""),
        ("deps/node_modules/m.js", ""),
        ("third/t.rs", ""),
        ("gen/g.rs", ""),
        (
            ".gitattributes",
            "third/** linguist-vendored\ngen/** linguist-generated\n",
        ),
    ]);

    let found = listed(dir.path(), &["fixtures/", "node_modules/"]);

    assert_eq!(
        found,
        [
            ".gitattributes",
            ".github/ci.yaml",
            ".gitignore",
            ".lighthouse/decisions/d.yaml",
            "gen/g.rs",
            "other/local.rs",
            "pkg/.gitignore",
            "pkg/kept.rs",
            "src/.lighthouse/cache/x",
            "src/a.rs",
        ]
    );
}

#[test]
fn a_later_pattern_wins_and_can_bring_a_file_back() {
    let dir = tree(&[("a/x.rs", ""), ("a/keep.rs", ""), ("b/y.rs", "")]);

    assert_eq!(
        listed(dir.path(), &["a/*", "!a/keep.rs"]),
        ["a/keep.rs", "b/y.rs"]
    );
}

#[test]
fn contains_judges_a_path_that_need_not_exist() {
    let dir = tree(&[(".gitignore", "target/\n"), ("src/.gitignore", "gen.rs\n")]);
    let files = ProjectFiles::new(dir.path(), &["fixtures/".to_owned()]).unwrap();

    for (path, expected) in [
        ("src/new.rs", true),
        ("target/debug/x", false),
        ("src/gen.rs", false),
        ("gen.rs", true),
        (".lighthouse/cache/x", false),
        (".lighthouse/decisions/x.yaml", true),
        ("fixtures/f.rs", false),
        (".git/config", false),
    ] {
        assert_eq!(files.contains(Path::new(path)), expected, "{path}");
    }
}

#[test]
fn vendored_means_left_out_and_generated_means_generated_with_no_cross_talk() {
    let dir = tree(&[
        ("a.rs", ""),
        ("v/x.rs", ""),
        ("g/x.rs", ""),
        ("both/x.rs", ""),
        (
            ".gitattributes",
            "v/** linguist-vendored\ng/** linguist-generated\nboth/** linguist-vendored linguist-generated\n",
        ),
    ]);

    let listing = ProjectFiles::new(dir.path(), &[]).unwrap().list();

    let names: Vec<_> = listing.files.iter().map(|f| f.to_str().unwrap()).collect();
    assert_eq!(names, [".gitattributes", "a.rs", "g/x.rs"]);
    assert!(listing.attributes.is_generated(Path::new("g/x.rs")));
    assert!(!listing.attributes.is_vendored(Path::new("g/x.rs")));
    assert!(listing.attributes.is_vendored(Path::new("v/x.rs")));
    assert!(!listing.attributes.is_generated(Path::new("v/x.rs")));
    assert!(listing.attributes.is_generated(Path::new("both/x.rs")));
    assert!(listing.attributes.is_vendored(Path::new("both/x.rs")));
}

#[test]
fn nested_gitattributes_are_relative_to_their_directory_and_deeper_files_win_per_attribute() {
    let dir = tree(&[
        (
            ".gitattributes",
            "*.pb.go linguist-generated\nlib/** linguist-vendored\n",
        ),
        (
            "sub/.gitattributes",
            "*.pb.go -linguist-generated\n/only.go linguist-generated\n",
        ),
        ("lib/inner/.gitattributes", "keep.go -linguist-vendored\n"),
    ]);
    let files: Vec<_> = [
        ".gitattributes",
        "sub/.gitattributes",
        "lib/inner/.gitattributes",
    ]
    .map(Into::into)
    .to_vec();

    let (attributes, notices) = Attributes::new(dir.path(), &files);

    assert!(notices.is_empty());
    assert!(attributes.is_generated(Path::new("a.pb.go")));
    assert!(
        !attributes.is_generated(Path::new("sub/a.pb.go")),
        "deeper wins"
    );
    assert!(attributes.is_generated(Path::new("sub/only.go")));
    assert!(
        !attributes.is_generated(Path::new("sub/deeper/only.go")),
        "anchored to its directory"
    );
    assert!(attributes.is_vendored(Path::new("lib/x.go")));
    assert!(!attributes.is_vendored(Path::new("lib/inner/keep.go")));
    assert!(
        attributes.is_vendored(Path::new("lib/inner/other.go")),
        "vendored is its own attribute"
    );
}

#[test]
fn extends_inherits_the_ignores_of_the_projects_it_names_first() {
    let base = Resource::new(
        Metadata::named("base"),
        ProjectSpec {
            ignores: vec!["gen/".to_owned(), "a/".to_owned()],
            ..ProjectSpec::default()
        },
    );
    let projects = Projects::new([base]).unwrap();
    let own = Config::parse_inline("extends = [\"base\"]\nignores = [\"a/\", \"own/\"]\n").unwrap();

    let merged = own.inheriting(&projects).unwrap();

    assert_eq!(merged.ignores(), ["gen/", "a/", "own/"]);
    assert_eq!(
        merged.inheriting(&projects).unwrap().ignores(),
        merged.ignores(),
        "resolving again changes nothing"
    );
}

#[test]
fn a_pattern_that_is_not_a_gitignore_line_is_an_error() {
    assert!(Config::parse_inline("ignores = [\"{a\"]\n").is_err());
}

#[test]
fn at_applies_the_same_patterns_to_another_root() {
    let one = tree(&[("out/a.rs", ""), ("b.rs", "")]);
    let two = tree(&[("out/c.rs", ""), ("d.rs", "")]);
    let files = ProjectFiles::new(one.path(), &["out/".to_owned()]).unwrap();

    let copy = files.at(two.path());

    assert_eq!(copy.root(), two.path());
    let listed: Vec<_> = copy.list().files;
    assert_eq!(listed, [Path::new("d.rs")]);
}

#[test]
fn root_is_where_the_files_are_listed_from() {
    let dir = tree(&[]);

    assert_eq!(
        ProjectFiles::new(dir.path(), &[]).unwrap().root(),
        dir.path()
    );
}

#[test]
fn matcher_reads_gitignore_lines_in_order_and_rejects_what_is_not_one() {
    let patterns = ["a/*".to_owned(), "!a/keep".to_owned()];

    let matcher = lighthouse_spec::ignore_matcher(Path::new("/p"), &patterns).unwrap();

    assert!(matcher.matched(Path::new("a/x"), false).is_ignore());
    assert!(!matcher.matched(Path::new("a/keep"), false).is_ignore());
    assert!(lighthouse_spec::ignore_matcher(Path::new("/p"), &["{a".to_owned()]).is_err());
}
