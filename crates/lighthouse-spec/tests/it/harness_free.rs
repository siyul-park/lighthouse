//! The core knows no agent harness: what an agent's stream means is
//! configuration (`recipes`), never code. Harness names appear only in the
//! recipes, the docs and tests.

use std::{fs, path::Path};

/// The harnesses with recipes.
const HARNESSES: [&str; 4] = ["opencode", "claude", "codex", "gemini"];

/// The crates that run an agent and report what it did. `lighthouse-cli` is
/// left out: it is the surface, and adapts Claude Code's hooks.
const CORE: [&str; 10] = [
    "lighthouse-engine",
    "lighthouse-model",
    "lighthouse-plugin",
    "lighthouse-process",
    "lighthouse-protocol",
    "lighthouse-report",
    "lighthouse-rpc",
    "lighthouse-session",
    "lighthouse-spec",
    "lighthouse-store",
];

/// The source files below `dir`, tests aside.
fn sources(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, found);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            found.push(path);
        }
    }
}

#[test]
fn no_harness_is_named_in_the_core_crates() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for name in CORE {
        sources(&crates.join(name).join("src"), &mut files);
    }
    assert!(files.len() > 50, "found only {} files", files.len());
    let named: Vec<String> = files
        .iter()
        .filter_map(|file| {
            let text = fs::read_to_string(file).unwrap().to_lowercase();
            let word = HARNESSES.iter().find(|word| text.contains(*word))?;
            Some(format!("{} names `{word}`", file.display()))
        })
        .collect();
    assert!(named.is_empty(), "{}", named.join("\n"));
}
