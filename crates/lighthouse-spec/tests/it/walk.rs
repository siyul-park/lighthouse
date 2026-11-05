//! The incremental walk gives what the `ignore` crate's walker gives, over
//! random edits of a file system.

use std::{
    fs,
    path::{Path, PathBuf},
};

use lighthouse_spec::{Disk, ProjectFiles};

/// A seeded generator: the sequences are the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

const NAMES: [&str; 6] = ["a", "b", "vendor", "out", ".hidden", "node_modules"];
const FILES: [&str; 6] = [
    "x.rs",
    "y.txt",
    "z.tmp",
    ".gitignore",
    ".gitattributes",
    "keep",
];

fn pick<'a>(rng: &mut Rng, from: &[&'a str]) -> &'a str {
    from[rng.below(from.len())]
}

fn some_dir(rng: &mut Rng, root: &Path) -> PathBuf {
    let mut at = root.to_path_buf();
    for _ in 0..rng.below(4) {
        at.push(pick(rng, &NAMES));
    }
    at
}

fn rule(rng: &mut Rng) -> &'static str {
    [
        "*.tmp\n",
        "out/\n",
        "/a/b/\n",
        "!keep\n*\n",
        "vendor\n",
        "x.rs\n",
    ][rng.below(6)]
}

fn attribute(rng: &mut Rng) -> &'static str {
    [
        "vendor/** linguist-vendored\n",
        "*.txt linguist-generated\n",
        "a/** linguist-vendored=false\n",
        "\n",
    ][rng.below(4)]
}

fn edit(rng: &mut Rng, root: &Path, counter: u64) {
    let dir = some_dir(rng, root);
    match rng.below(9) {
        0..=2 => {
            let _ = fs::create_dir_all(&dir);
            let file = dir.join(pick(rng, &FILES));
            let text = match file.file_name().and_then(|n| n.to_str()) {
                Some(".gitignore") => rule(rng).to_owned(),
                Some(".gitattributes") => attribute(rng).to_owned(),
                _ => format!("v{counter}"),
            };
            let _ = fs::write(file, text);
        }
        3 => {
            let _ = fs::remove_file(dir.join(pick(rng, &FILES)));
        }
        4 => {
            let _ = fs::remove_dir_all(&dir);
        }
        5 => {
            let _ = fs::create_dir_all(dir.join(pick(rng, &NAMES)));
        }
        6 => {
            let _ = fs::rename(dir.join(pick(rng, &FILES)), dir.join(pick(rng, &FILES)));
        }
        7 => {
            let _ = fs::create_dir_all(dir.join(".git/info"));
            let _ = fs::write(dir.join(".git/info/exclude"), rule(rng));
        }
        _ => {
            #[cfg(unix)]
            {
                let _ = fs::create_dir_all(&dir);
                let _ = std::os::unix::fs::symlink(root, dir.join("link"));
            }
        }
    }
}

#[test]
fn the_walk_equals_the_reference_walker_over_random_edits() {
    for seed in 1..=12u64 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project =
            ProjectFiles::new(&root, &["*.log".to_owned(), "/a/skip/".to_owned()]).unwrap();
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for step in 0..60 {
            for _ in 0..1 + rng.below(3) {
                edit(&mut rng, &root, step);
            }
            let walked = project.walk(&mut Disk(&root));
            let reference = project.walker_list();
            assert_eq!(
                walked.listing.files, reference.files,
                "seed {seed} step {step}"
            );
            assert_eq!(
                walked.listing.notices, reference.notices,
                "seed {seed} step {step}"
            );
            for file in &walked.listing.files {
                assert!(walked.stats.contains_key(file), "{file:?} has a stat");
            }
            assert_eq!(walked.stats.len(), walked.listing.files.len());
        }
    }
}

#[test]
fn a_stat_is_racy_within_the_clock_tick_of_the_walk() {
    let stat = |mtime_ns| lighthouse_spec::Stat {
        mtime_ns,
        ctime_ns: 0,
        size: 0,
        inode: 0,
        mode: 0,
    };
    let started = 10 * lighthouse_spec::RACY_NS;

    assert!(stat(started).racy(started));
    assert!(stat(started - lighthouse_spec::RACY_NS).racy(started));
    assert!(!stat(started - lighthouse_spec::RACY_NS - 1).racy(started));
}

#[test]
fn stat_tells_a_file_that_was_rewritten_from_one_that_was_not() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, "one").unwrap();
    let stat = |path: &Path| lighthouse_spec::Stat::of(&fs::symlink_metadata(path).unwrap());
    let before = stat(&file);
    assert_eq!(before, stat(&file));

    fs::write(&file, "three").unwrap();

    assert_ne!(before, stat(&file));
    assert_eq!(stat(&file).size, 5);
}

#[test]
fn read_entries() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("b.txt"), "x").unwrap();
    fs::write(dir.path().join("a.txt"), "x").unwrap();

    let entries = lighthouse_spec::read_entries(dir.path());
    let names: Vec<_> = entries
        .raw
        .iter()
        .map(|raw| raw.name.to_str().unwrap())
        .collect();
    let missing = lighthouse_spec::read_entries(&dir.path().join("none"));

    assert_eq!(names, ["a.txt", "b.txt", "sub"], "sorted by name");
    assert_eq!(entries.raw[2].kind, lighthouse_spec::Kind::Dir);
    assert!(entries.errors.is_empty());
    assert!(missing.raw.is_empty() && !missing.errors.is_empty());
}
