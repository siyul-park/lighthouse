//! Failure handling and layout rules of the provider that golden fixtures
//! cannot show: a broken file must be reported as a gap, never skipped.

use std::{
    fs,
    io::BufReader,
    path::Path,
    process::{Command, Stdio},
};

use lighthouse_protocol::{
    self as wire, FileRef, IndexParams, IndexResult, Message, read_message, write_message,
};
use tempfile::TempDir;

fn project(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
    dir
}

fn index(dir: &Path, options: serde_json::Value) -> IndexResult {
    let root = dir.canonicalize().unwrap();
    let mut paths = Vec::new();
    collect(&root, &root, &mut paths);
    paths.sort();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lang-rust"))
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let params = IndexParams {
        project: wire::ProjectRef {
            root: root.to_string_lossy().into_owned(),
        },
        language: "rust".to_owned(),
        files: paths
            .iter()
            .map(|path| FileRef {
                path: path.clone(),
                hash: String::new(),
            })
            .collect(),
        context: wire::Context {
            options: serde_json::from_value(options).unwrap(),
            overlays: None,
            cache: None,
        },
    };
    let request = Message::request(1, wire::INDEX, params).unwrap();
    write_message(&mut stdin, &request).unwrap();
    let reply = read_message(&mut stdout).unwrap().unwrap();
    let result = reply.into_result::<IndexResult>().unwrap().unwrap();
    write_message(
        &mut stdin,
        &Message::request(2, wire::SHUTDOWN, ()).unwrap(),
    )
    .unwrap();
    read_message(&mut stdout).unwrap().unwrap();
    write_message(&mut stdin, &Message::notification(wire::EXIT)).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    result
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path.strip_prefix(root).unwrap();
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

const MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";

fn reasons(result: &IndexResult) -> Vec<String> {
    result
        .incomplete
        .iter()
        .map(|i| format!("{}: {}", i.path.as_deref().unwrap_or("-"), i.reason))
        .collect()
}

#[test]
fn a_file_that_does_not_parse_is_a_gap_with_its_position_and_still_has_a_fragment() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub mod broken;\npub fn fine() {}\n"),
        ("src/broken.rs", "pub fn oops( {\n"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    let reasons = reasons(&result);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].starts_with("src/broken.rs: 1:"), "{reasons:?}");
    let paths: Vec<_> = result
        .fragments
        .iter()
        .map(|f| f.file.path.as_str())
        .collect();
    assert_eq!(paths, ["src/broken.rs", "src/lib.rs"]);
    assert_eq!(result.fragments[1].symbols.len(), 1);
}

#[test]
fn a_mod_without_a_file_is_a_gap_on_the_declaring_file() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "mod missing;\npub fn fine() {}\n"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    let reasons = reasons(&result);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(
        reasons[0].contains("src/lib.rs: `mod missing;` has no file"),
        "{reasons:?}"
    );
}

#[test]
fn a_file_no_crate_target_reaches_is_a_notice_not_a_gap() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub fn fine() {}\n"),
        ("src/orphan.rs", "pub fn lost() {}\n"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    assert!(
        result.notices.iter().any(|n| n.contains("src/orphan.rs")),
        "{:?}",
        result.notices
    );
    let orphan = result
        .fragments
        .iter()
        .find(|f| f.file.path == "src/orphan.rs")
        .unwrap();
    assert!(orphan.symbols.is_empty());
}

#[test]
fn an_orphan_beside_a_parse_error_is_a_gap_because_the_tree_may_be_cut_short() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "mod broken;\n"),
        ("src/broken.rs", "fn ( {\nmod inner;\n"),
        ("src/broken/inner.rs", "pub fn lost() {}\n"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    let reasons = reasons(&result);
    assert!(
        reasons
            .iter()
            .any(|r| r.starts_with("src/broken/inner.rs: not reached")),
        "{reasons:?}"
    );
}

#[test]
fn files_without_a_manifest_form_one_synthetic_package() {
    let dir = project(&[
        ("src/lib.rs", "mod util;\npub fn run() { util::help(); }\n"),
        ("src/util.rs", "pub fn help() {}\n"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let lib = &result.fragments[0];
    assert_eq!(lib.symbols[0].id, "workspace::run#function");
    assert_eq!(lib.edges.len(), 2, "{:?}", lib.edges);
}

#[test]
fn a_manifest_that_does_not_parse_is_a_gap_for_its_files() {
    let dir = project(&[
        ("Cargo.toml", "[package\n"),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    let reasons = reasons(&result);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].starts_with("src/lib.rs: "), "{reasons:?}");
}

#[test]
fn unknown_option_keys_are_rejected_as_a_gap() {
    let dir = project(&[("Cargo.toml", MANIFEST), ("src/lib.rs", "")]);
    let result = index(
        dir.path(),
        serde_json::json!({ "rust": { "cfg": ["test"] } }),
    );
    assert!(result.fragments.is_empty());
    assert!(result.incomplete[0].reason.contains("cfg"));
}

#[test]
fn testdata_and_hidden_directories_are_skipped_with_empty_fragments() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub fn a() {}\n"),
        ("testdata/bad.rs", "fn ( {"),
        (".hidden/bad.rs", "fn ( {"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    assert_eq!(result.fragments.len(), 3);
    assert!(
        result
            .fragments
            .iter()
            .filter(|f| f.file.path != "src/lib.rs")
            .all(|f| f.symbols.is_empty())
    );
}

#[test]
fn path_attributes_and_nested_inline_modules_find_their_files() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "mod a { #[path = \"x.rs\"] mod b; mod c; }\n#[path = \"../shared/s.rs\"]\nmod s;\n",
        ),
        ("src/a/x.rs", "pub fn in_b() {}\n"),
        ("src/a/c.rs", "pub fn in_c() {}\n"),
        ("shared/s.rs", "pub fn in_s() {}\n"),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let ids: Vec<&str> = result
        .fragments
        .iter()
        .flat_map(|f| f.symbols.iter().map(|s| s.id.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            "demo/s::in_s#function",
            "demo/a/c::in_c#function",
            "demo/a/b::in_b#function"
        ]
    );
}

#[test]
fn columns_count_bytes_not_characters() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "pub const GREETING: &str = \"héllo\"; pub fn after() {}\n",
        ),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    let after = result.fragments[0]
        .symbols
        .iter()
        .find(|s| s.name == "after")
        .unwrap();
    let line = "pub const GREETING: &str = \"héllo\"; pub fn after() {}";
    let at = line.find("pub fn").unwrap() + 1;
    assert_eq!(after.span.start.col as usize, at);
    assert_eq!(after.span.end.col as usize, line.len() + 1);
}

#[test]
fn a_static_initializer_uses_what_it_names_and_enum_variants_are_variants() {
    let dir = project(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "pub enum Mode { Fast, Slow }\nfn run() {}\nstatic HANDLERS: &[fn()] = &[run];\n",
        ),
    ]);
    let result = index(dir.path(), serde_json::json!({}));
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let fragment = &result.fragments[0];
    let variants: Vec<&str> = fragment
        .symbols
        .iter()
        .filter(|s| s.kind == wire::SymbolKind::Variant)
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(
        variants,
        ["demo::Mode::Fast#variant", "demo::Mode::Slow#variant"]
    );
    let uses_run = fragment.edges.iter().any(|e| {
        matches!(&e.from, wire::Node::Symbol(from) if from == "demo::HANDLERS#var")
            && e.to == "demo::run#function"
    });
    assert!(uses_run, "{:?}", fragment.edges);
}
