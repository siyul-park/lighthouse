//! Generated code, told apart by the host from the head comments the language
//! plugins report: the fixtures the plugins' conformance suites keep.

use lighthouse_model::ucm::Model;
use std::{fs, path::Path};

use lighthouse_engine::Engine;
use lighthouse_spec::Config;

const GO: [&str; 4] = ["gen.go", "gogo.go", "notgen.go", "plain.go"];
const TS: [&str; 4] = ["gen.ts", "marked.ts", "mention.ts", "plain.ts"];
const PY: [&str; 4] = ["late.py", "marked.py", "message_pb2.py", "plain.py"];
const RS: [&str; 3] = ["src/gen.rs", "src/lib.rs", "src/plain.rs"];
const CONFORMANCE: &str = "../lighthouse-rpc/tests/conformance";

/// Which of the files of the conformance case `case` the engine of the
/// plugin `plugin` (at `dir`, listed as `id`) takes for generated, with
/// `extra` as the project's own configuration.
fn generated(lang: &str, id: &str, plugin: &Path, extra: &str, files: &[&str]) -> Vec<String> {
    let project = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(CONFORMANCE)
            .join(lang)
            .join("generated/project"),
        project.path(),
    );
    let config = Config::parse_inline(&format!(
        "plugins = [{{ id = \"{id}\", path = {:?} }}]\n{extra}",
        plugin.display().to_string()
    ))
    .unwrap();
    let registry = lighthouse_test_support::pooled(lighthouse_test_support::registry, &config)();
    let config = config.enabling(registry.manifests().iter().map(|m| m.id.as_str()));
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        project.path(),
    )
    .unwrap();

    let run = engine.check_project(&[], &[]).unwrap();

    files
        .iter()
        .filter(|file| {
            run.project
                .file(Path::new(file))
                .is_some_and(|f| f.generated)
        })
        .map(|file| (*file).to_owned())
        .collect()
}

fn copy(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn go_files_with_a_generator_header_are_generated_and_files_that_mention_one_are_not() {
    let Some(plugin) = lighthouse_test_support::lang_go() else {
        return;
    };

    assert_eq!(
        generated("go", "lang-go", &plugin, "", &GO),
        ["gen.go", "gogo.go"]
    );
}

#[test]
fn typescript_files_marked_in_their_head_are_generated() {
    let Some(plugin) = lighthouse_test_support::lang_typescript() else {
        return;
    };

    assert_eq!(
        generated("typescript", "lang-typescript", &plugin, "", &TS),
        ["gen.ts", "marked.ts"]
    );
}

#[test]
fn python_files_marked_in_their_head_are_generated_and_a_late_comment_is_not() {
    let Some(plugin) = lighthouse_test_support::lang_python() else {
        return;
    };

    assert_eq!(
        generated("python", "lang-python", &plugin, "", &PY),
        ["marked.py", "message_pb2.py"]
    );
}

#[test]
fn a_marker_of_the_project_marks_what_the_defaults_miss_in_any_language() {
    let Some(plugin) = lighthouse_test_support::lang_python() else {
        return;
    };
    let extra = "[generated]\nmarkers = ['Plain is written']\nfiles = ['plain.py']\n";

    let found = generated("python", "lang-python", &plugin, extra, &PY);

    assert_eq!(found, ["marked.py", "message_pb2.py", "plain.py"]);
}

#[test]
fn rust_files_marked_in_their_head_are_generated() {
    let plugin = lighthouse_test_support::lang_rust();

    assert_eq!(
        generated("rust", "lang-rust", &plugin, "", &RS),
        ["src/gen.rs"]
    );
}
