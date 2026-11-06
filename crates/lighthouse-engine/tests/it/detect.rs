//! Detecting files through the graph: after any sequence of edits to the file
//! system the files and their hashes are those of a fresh walk, the findings
//! are those of a run without a cache, and a file is read only when it moved.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use lighthouse_engine::Run;
use lighthouse_protocol::hash;
use lighthouse_spec::ProjectFiles;

use super::cache::engine_with;

const LIMIT: u64 = 64 << 20;

fn config(ignores: &str) -> String {
    format!(
        "plugins = [\"counting\"]\nignores = [{ignores}]\n[rules]\n\"counting/local\" = \"warn\"\n\"counting/global\" = \"warn\"\n"
    )
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

fn run_with(root: &Path, cache: Option<&Path>, ignores: &str) -> Run {
    let (engine, _) = engine_with(root, &config(ignores));
    let engine = match cache {
        Some(dir) => engine.with_cache(dir.to_owned(), LIMIT),
        None => engine,
    };
    engine.check_project(&[], &[]).expect("check")
}

fn found(run: &Run) -> Vec<(String, String)> {
    run.diagnostics
        .iter()
        .map(|d| (d.file.display().to_string(), d.message.clone()))
        .collect()
}

/// The files and hashes a fresh walk and a fresh hash give.
fn fresh(root: &Path, patterns: &[String]) -> Vec<(PathBuf, String)> {
    ProjectFiles::new(root, patterns)
        .expect("patterns")
        .walker_list()
        .files
        .into_iter()
        .filter_map(|rel| {
            let text = fs::read_to_string(root.join(&rel)).ok()?;
            Some((rel, hash::sha256(text)))
        })
        .collect()
}

fn hashes(run: &Run) -> Vec<(PathBuf, String)> {
    run.read
        .iter()
        .map(|(path, hash)| (path.clone(), hash.clone()))
        .collect()
}

fn write(root: &Path, rel: &str, text: &str) {
    let at = root.join(rel);
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    fs::write(at, text).unwrap();
}

/// One edit; the names are few, so that they meet.
fn edit(rng: &mut Rng, root: &Path, step: u64) -> String {
    let dirs = ["", "a/", "a/b/", "c/"];
    let names = ["x.txt", "y.txt", "z.skip", "w.txt"];
    let path = format!("{}{}", dirs[rng.below(4)], names[rng.below(4)]);
    let at = root.join(&path);
    match rng.below(9) {
        0 => {
            if let Ok(file) = File::options().write(true).open(&at) {
                file.set_modified(SystemTime::now() + Duration::from_secs(step))
                    .unwrap();
            }
            "touch".into()
        }
        1 | 2 => {
            write(root, &path, &format!("edit {step}"));
            "edit".into()
        }
        3 => {
            write(root, &format!("{}new{step}.txt", dirs[rng.below(4)]), "new");
            "add".into()
        }
        4 => {
            let _ = fs::remove_file(&at);
            "remove".into()
        }
        5 => {
            let to = root.join(format!("{}moved{step}.txt", dirs[rng.below(4)]));
            if to.parent().is_some_and(Path::exists) {
                let _ = fs::rename(&at, to);
            }
            "rename".into()
        }
        6 => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if at.exists() {
                    let mode = if rng.below(2) == 0 { 0o755 } else { 0o644 };
                    fs::set_permissions(&at, fs::Permissions::from_mode(mode)).unwrap();
                }
            }
            "chmod".into()
        }
        7 => {
            let rule = ["*.skip\n", "a/b/\n", "c/\n", "\n"][rng.below(4)];
            write(root, &format!("{}.gitignore", dirs[rng.below(4)]), rule);
            "gitignore".into()
        }
        _ => {
            // Rewritten at once with text of the same size: the racy case.
            write(root, &path, "same-1");
            let first = run_with(root, None, "");
            write(root, &path, "same-2");
            drop(first);
            "racy".into()
        }
    }
}

#[test]
fn files_and_findings_equal_a_fresh_walk_over_random_edits() {
    for seed in 1..=4u64 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let cache = tempfile::tempdir().unwrap();
        for rel in ["x.txt", "a/x.txt", "a/b/y.txt", "c/w.txt"] {
            write(&root, rel, rel);
        }
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut ignores = String::new();
        for step in 0..30u64 {
            let what = edit(&mut rng, &root, step);
            if step % 10 == 9 {
                ignores = if ignores.is_empty() {
                    "\"w.txt\"".into()
                } else {
                    String::new()
                };
            }
            let patterns: Vec<String> = if ignores.is_empty() {
                vec![]
            } else {
                vec!["w.txt".to_owned()]
            };
            let warm = run_with(&root, Some(cache.path()), &ignores);
            let plain = run_with(&root, None, &ignores);
            assert_eq!(
                hashes(&warm),
                fresh(&root, &patterns),
                "seed {seed} step {step} {what}"
            );
            assert_eq!(
                found(&warm),
                found(&plain),
                "seed {seed} step {step} {what}"
            );
        }
    }
}

#[test]
fn a_run_with_nothing_changed_reads_no_file_and_has_the_same_root_hash() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cache = tempfile::tempdir().unwrap();
    for rel in ["x.txt", "a/y.txt", "a/b/z.txt"] {
        write(&root, rel, rel);
    }
    let first = run_with(&root, Some(cache.path()), "");
    assert_eq!(first.timings.files.read, 3);
    std::thread::sleep(Duration::from_millis(2200));
    // The files were written within the clock's tick of the first run: the
    // second reads them again, once, and the third has nothing to read.
    let second = run_with(&root, Some(cache.path()), "");
    let third = run_with(&root, Some(cache.path()), "");

    assert_eq!(second.timings.files.read, 3, "{:?}", second.timings.files);
    assert_eq!(third.timings.files.read, 0, "{:?}", third.timings.files);
    assert_eq!(third.timings.files.entries, 0);
    assert_eq!(first.timings.files.tree, third.timings.files.tree);

    write(&root, "a/y.txt", "changed");
    let fourth = run_with(&root, Some(cache.path()), "");
    assert_eq!(fourth.timings.files.read, 1);
    assert_ne!(fourth.timings.files.tree, third.timings.files.tree);
}

#[test]
fn an_edit_in_the_same_second_as_the_last_run_is_seen() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(&root, "x.txt", "same-1");
    run_with(&root, Some(cache.path()), "");
    write(&root, "x.txt", "same-2");

    let run = run_with(&root, Some(cache.path()), "");

    assert_eq!(run.read[Path::new("x.txt")], hash::sha256("same-2"));
}

#[test]
fn an_overlay_stands_in_for_the_file_and_gives_way_when_it_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(&root, "x.txt", "on disk");
    write(&root, "y.txt", "other");
    let (engine, _) = engine_with(&root, &config(""));
    let engine = engine.with_cache(cache.path().to_owned(), LIMIT);
    let hash_of = |run: &Run, name: &str| run.read[Path::new(name)].clone();
    let overlaid = |text: &str| -> lighthouse_engine::Overlays {
        [(PathBuf::from("x.txt"), text.to_owned())].into()
    };

    // Files this new are read again at the next run; two runs settle them.
    engine.check_project(&[], &[]).unwrap();
    std::thread::sleep(Duration::from_millis(2200));
    engine.check_project(&[], &[]).unwrap();
    let plain = engine.check_project(&[], &[]).unwrap();
    let first = engine.check_overlaid(&[], &[], &overlaid("one")).unwrap();
    let second = engine.check_overlaid(&[], &[], &overlaid("two")).unwrap();
    let again = engine.check_overlaid(&[], &[], &overlaid("one")).unwrap();
    let dropped = engine.check_project(&[], &[]).unwrap();

    assert_eq!(hash_of(&plain, "x.txt"), hash::sha256("on disk"));
    assert_eq!(hash_of(&first, "x.txt"), hash::sha256("one"));
    assert_eq!(hash_of(&second, "x.txt"), hash::sha256("two"));
    assert_eq!(hash_of(&again, "x.txt"), hash::sha256("one"));
    assert_eq!(hash_of(&dropped, "x.txt"), hash::sha256("on disk"));
    assert_eq!(hash_of(&first, "y.txt"), hash::sha256("other"));
}

#[test]
fn runs_from_several_threads_each_see_their_own_overlays() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cache = tempfile::tempdir().unwrap();
    for name in ["x.txt", "y.txt", "z.txt"] {
        write(&root, name, name);
    }
    let (engine, _) = engine_with(&root, &config(""));
    let engine = engine.with_cache(cache.path().to_owned(), LIMIT);
    std::thread::scope(|scope| {
        for n in 0..8 {
            let engine = &engine;
            scope.spawn(move || {
                for round in 0..10 {
                    let text = format!("t{n}-{round}");
                    let overlays: lighthouse_engine::Overlays = [(
                        PathBuf::from(["x.txt", "y.txt", "z.txt"][(n + round) % 3]),
                        text.clone(),
                    )]
                    .into();
                    let run = engine.check_overlaid(&[], &[], &overlays).unwrap();
                    let (path, text) = overlays.iter().next().unwrap();
                    let seen = run.read[path].clone();
                    assert_eq!(seen, hash::sha256(text), "thread {n} round {round}");
                }
            });
        }
    });
}
