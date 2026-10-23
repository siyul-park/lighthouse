//! The events of an agent's stdout, through the mapping a recipe declares: one
//! fixture stream per recipe, run through the generic CEL path, against the
//! events and the words it must give. The recipes are configuration layers of
//! the repository's `recipes` directory; the host knows none of them. Then the
//! mapping itself: streams past the caps, lines that fail, several events to a
//! line.

use std::{fs, path::Path};

use lighthouse_plugin::{EditRequest, Edited, ModelClient, Tool, ToolEvent};
use lighthouse_process::Policy;
use lighthouse_rpc::CommandModel;
use lighthouse_spec::{Layer, ModelProvider, Program};

const RECIPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../recipes");

/// The provider declared by the layer `text` as `name`, with the command
/// swapped for one that prints `stream`.
fn provider_of(text: &str, name: &str, stream: &Path) -> ModelProvider {
    let layer = Layer::parse(&format!("{name}.toml"), text).unwrap();
    let ModelProvider::Command {
        protocol,
        program,
        pointer,
        events,
        cost,
    } = layer.spec.models.providers[name].resolve().unwrap()
    else {
        panic!("{name} is not a command provider");
    };
    assert!(events.is_some(), "{name} declares no events");
    ModelProvider::Command {
        protocol,
        program: Program {
            command: vec!["cat".to_owned(), stream.display().to_string()],
            ..program
        },
        pointer,
        events,
        cost,
    }
}

/// What the layer `text` makes of `stream` (with `@ROOT@` the directory the
/// agent ran in), run in a directory that holds `files`.
fn run_with(text: &str, name: &str, stream: &str, files: &[&str]) -> Edited {
    let root = tempfile::tempdir().unwrap();
    for file in files {
        let path = root.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x\n").unwrap();
    }
    let kept = tempfile::tempdir().unwrap();
    let path = kept.path().join("stream.jsonl");
    fs::write(
        &path,
        stream.replace("@ROOT@", &root.path().display().to_string()),
    )
    .unwrap();
    let provider = provider_of(text, name, &path);
    let agent = CommandModel::new(
        &lighthouse_test_support::cel(),
        kept.path(),
        name,
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();
    agent
        .edit(&EditRequest {
            prompt: "fix it",
            dir: root.path(),
            model: "",
        })
        .unwrap()
}

/// What the layer `text` makes of `stream`, run in an empty directory.
fn run(text: &str, name: &str, stream: &str) -> Edited {
    run_with(text, name, stream, &[])
}

/// What the recipe `name` makes of its fixture stream, run in a directory
/// that holds the files its `files.txt` lists.
fn edited(name: &str) -> Edited {
    let text = fs::read_to_string(format!("{RECIPES}/{name}/recipe.toml")).unwrap();
    let stream = fs::read_to_string(format!("{RECIPES}/{name}/stream.jsonl")).unwrap();
    let files = fs::read_to_string(format!("{RECIPES}/{name}/files.txt")).unwrap();
    run_with(&text, name, &stream, &files.lines().collect::<Vec<_>>())
}

fn expected(name: &str) -> Vec<ToolEvent> {
    let text = fs::read_to_string(format!("{RECIPES}/{name}/events.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn said(name: &str) -> String {
    let text = fs::read_to_string(format!("{RECIPES}/{name}/said.txt")).unwrap();
    text.trim_end().to_owned()
}

#[test]
fn the_opencode_recipe_maps_its_stream_to_events() {
    let found = edited("opencode");

    assert_eq!(found.events, expected("opencode"));
    assert_eq!(found.said, said("opencode"));
    assert_eq!(found.incomplete, None);
}

#[test]
fn the_claude_code_recipe_maps_its_stream_to_events() {
    let found = edited("claude-code");

    assert_eq!(found.events, expected("claude-code"));
    assert_eq!(found.said, said("claude-code"));
}

#[test]
fn the_codex_recipe_maps_its_stream_to_events() {
    let found = edited("codex");

    assert_eq!(found.events, expected("codex"));
    assert_eq!(found.said, said("codex"));
}

#[test]
fn the_gemini_cli_recipe_maps_its_stream_to_events() {
    let found = edited("gemini-cli");

    assert_eq!(found.events, expected("gemini-cli"));
    assert_eq!(found.said, said("gemini-cli"));
}

#[test]
fn every_recipe_directory_has_a_golden_test() {
    let mut names: Vec<String> = fs::read_dir(RECIPES)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["claude-code", "codex", "gemini-cli", "opencode"]);
}

/// A layer that maps lines of `{"name": ..., "file": ...}`.
fn layer(mapping: &str) -> String {
    format!(
        "[models.providers.p]\ncommand = [\"harness\"]\n[models.providers.p.events]\n\
         select = 'has(line.name)'\ntool = 'tools[line.name]'\n\
         tools = {{ cat = \"read\", rg = \"search\" }}\n{mapping}"
    )
}

#[test]
fn a_stream_larger_than_the_output_cap_is_read_to_its_last_event() {
    // 2000 lines of 2 KiB: several times the megabyte a run keeps of its output.
    let pad = "x".repeat(2048);
    let stream: String = (0..2000)
        .map(|n| format!("{{\"name\":\"cat\",\"file\":\"f{n}.go\",\"output\":\"{pad}\"}}\n"))
        .collect();
    assert!(stream.len() > 3 << 20);

    let found = run(
        &layer("pattern = 'line.file'\nsaid = 'line.file'"),
        "p",
        &stream,
    );

    assert_eq!(found.events.len(), 2000);
    assert_eq!(found.events[1999].pattern.as_deref(), Some("f1999.go"));
    assert_eq!(found.incomplete, None);
    assert!(found.said.ends_with("f1999.go"), "the end of it is kept");
    assert!(found.said.len() <= 2048);
}

#[test]
fn events_past_the_count_kept_are_counted_and_make_the_batch_incomplete() {
    let stream = "{\"name\":\"cat\"}\n".repeat(10_050);

    let found = run(&layer(""), "p", &stream);

    assert_eq!(found.events.len(), 10_000);
    let note = found.incomplete.unwrap();
    assert!(note.contains("at least 50 event(s)"), "{note}");
}

#[test]
fn a_line_longer_than_the_limit_is_dropped_and_makes_the_batch_incomplete() {
    let long = format!("{{\"name\":\"cat\",\"pad\":\"{}\"}}\n", "x".repeat(9 << 20));
    let stream = format!("{long}{{\"name\":\"rg\"}}\n");

    let found = run(&layer(""), "p", &stream);

    assert_eq!(found.events.len(), 1);
    assert_eq!(found.events[0].tool, Tool::Search);
    assert!(found.incomplete.is_some());
}

#[test]
fn a_line_that_fails_to_map_is_skipped_and_the_rest_are_read() {
    let stream = [
        "not json at all",
        "{\"name\":\"unknown-tool\"}",
        "{\"name\":\"cat\",\"file\":5}",
        "{\"name\":\"cat\",\"file\":\"a.go\"}",
        "",
    ]
    .join("\n");

    // `path` is an int on the third line: a runtime error is no path, and the
    // event stays.
    let found = run_with(&layer("path = 'line.file'"), "p", &stream, &["a.go"]);

    let seen: Vec<(Tool, Option<&str>)> = found
        .events
        .iter()
        .map(|e| (e.tool, e.path.as_deref()))
        .collect();
    assert_eq!(seen, [(Tool::Read, None), (Tool::Read, Some("a.go"))]);
    assert_eq!(found.incomplete, None);
}

#[test]
fn each_makes_an_event_of_every_element_and_a_list_of_paths_one_for_each_path() {
    let stream = "{\"name\":\"cat\",\"files\":[\"a.go\",\"b.go\"]}\n";

    let found = run_with(
        &layer("each = 'line.files.map(f, {\"p\": f})'\npath = '[item.p, item.p + \".bak\"]'"),
        "p",
        stream,
        &["a.go", "a.go.bak", "b.go", "b.go.bak"],
    );

    let paths: Vec<_> = found.events.iter().map(|e| e.path.as_deref()).collect();
    assert_eq!(
        paths,
        [
            Some("a.go"),
            Some("a.go.bak"),
            Some("b.go"),
            Some("b.go.bak")
        ]
    );
}

#[test]
fn a_secret_the_stream_echoes_does_not_reach_the_events_or_what_was_said() {
    // SAFETY: this variable belongs to this test.
    unsafe { std::env::set_var("LIGHTHOUSE_RPC_TEST_EVENTS", "s3cret") };
    let stream = "{\"name\":\"cat\",\"file\":\"token-s3cret\"}\n";
    let text = layer("pattern = 'line.file'\nsaid = 'line.file'").replace(
        "command = [\"harness\"]",
        "command = [\"harness\"]\nenv = { KEY = \"${LIGHTHOUSE_RPC_TEST_EVENTS}\" }",
    );

    let found = run(&text, "p", stream);

    assert_eq!(
        found.events[0].pattern.as_deref(),
        Some("token-${LIGHTHOUSE_RPC_TEST_EVENTS}")
    );
    assert_eq!(found.said, "token-${LIGHTHOUSE_RPC_TEST_EVENTS}");
}

#[test]
fn an_operand_counts_as_a_path_only_when_it_names_an_existing_file_of_the_directory() {
    let stream = "{\"name\":\"cat\",\"operands\":[\"Makefile\",\"s/a/b/\",\"x.go\",\"gone.go\"]}\n";

    let found = run_with(
        &layer("path = 'line.operands'"),
        "p",
        stream,
        &["Makefile", "x.go"],
    );

    let paths: Vec<_> = found.events.iter().map(|e| e.path.as_deref()).collect();
    assert_eq!(paths, [Some("Makefile"), Some("x.go")]);
}

#[test]
fn a_call_whose_operands_name_no_file_is_one_event_without_a_path() {
    let stream = "{\"name\":\"cat\",\"operands\":[\"s/a/b/\",\"gone.go\"]}\n";

    let found = run(&layer("path = 'line.operands'"), "p", stream);

    assert_eq!(found.events.len(), 1);
    assert_eq!(found.events[0].path, None);
}

#[test]
fn a_secret_the_json_escapes_is_hidden_in_what_the_events_carry() {
    // SAFETY: this variable belongs to this test.
    unsafe { std::env::set_var("LIGHTHOUSE_RPC_TEST_ESCAPED", "s3cret/x") };
    let stream = "{\"name\":\"cat\",\"file\":\"token-s3cret\\/x\"}\n";
    let text = layer("pattern = 'line.file'\nsaid = 'line.file'").replace(
        "command = [\"harness\"]",
        "command = [\"harness\"]\nenv = { KEY = \"${LIGHTHOUSE_RPC_TEST_ESCAPED}\" }",
    );

    let found = run(&text, "p", stream);

    let pattern = found.events[0].pattern.as_deref().unwrap();
    assert!(!pattern.contains("s3cret"), "{pattern}");
    assert!(!found.said.contains("s3cret"), "{}", found.said);
}

#[test]
fn a_line_that_holds_more_calls_than_are_kept_builds_no_more_than_are_kept() {
    let files: Vec<String> = (0..30_000).map(|n| format!("\"f{n}\"")).collect();
    let stream = format!("{{\"name\":\"cat\",\"files\":[{}]}}\n", files.join(","));
    let text = layer("each = 'line.files.map(f, {\"p\": f})'\npattern = 'item.p'");

    let found = run(&text, "p", &stream);

    assert_eq!(found.events.len(), 10_000);
    assert!(found.incomplete.unwrap().contains("at least"));
}

#[test]
fn the_final_reply_after_far_more_events_than_are_kept_is_still_said() {
    let mut stream = "{\"name\":\"cat\"}\n".repeat(20_000);
    stream.push_str("{\"msg\":\"fixed: all\"}\n");
    let text = layer("said = 'has(line.msg) ? line.msg : \"\"'");

    let found = run(&text, "p", &stream);

    assert_eq!(found.events.len(), 10_000);
    assert_eq!(found.said, "fixed: all");
    assert!(found.incomplete.is_some());
}

#[test]
fn lines_far_past_the_event_limit_are_read_for_what_was_said_only_and_the_note_says_so() {
    let stream = "{\"name\":\"cat\"}\n".repeat(10_000 + 1_500);

    let found = run(&layer(""), "p", &stream);

    assert_eq!(found.events.len(), 10_000);
    let note = found.incomplete.unwrap();
    assert!(note.contains("500 line(s) past the event limit"), "{note}");
}
