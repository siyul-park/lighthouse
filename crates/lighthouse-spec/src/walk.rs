//! The walk of a project that remembers: the listing [`ProjectFiles`] gives,
//! with the `lstat` of every file, from a walk that reads a directory again
//! only when its own `lstat` moved.
//!
//! The rules are the `ignore` crate walker's, which this is tested against
//! ([`ProjectFiles::walker_list`]): `.gitignore` files from the root down
//! (the deepest match wins), the `info/exclude` of each repository found,
//! then [`ProjectFiles::enters`] and `.gitattributes`. They are applied to
//! what a directory holds on every walk; only the `readdir` is skipped. A
//! directory that did not change is one whose `lstat` is the same as at the
//! last walk and is not racy: git's untracked cache, and its racy rule.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

mod chain;

use crate::{Attributes, Listing, ProjectFiles};
use chain::{Level, ignored};

/// A directory whose modification time is this close to the start of the last
/// walk, or later, is read again: the file system's clock may be as coarse as
/// two seconds (FAT) and a change in the same tick is not told by the time.
pub const RACY_NS: i64 = 2_000_000_000;

struct Walk<'a> {
    project: &'a ProjectFiles,
    root: PathBuf,
    previous: &'a Dirs,
    since: Option<i64>,
    out: Walked,
    files: Vec<PathBuf>,
}

/// What a walk found.
#[derive(Default)]
pub struct Walked {
    pub listing: Listing,
    /// The `lstat` of each listed file.
    pub stats: BTreeMap<PathBuf, Stat>,
    /// For each directory entered, what it lists, sorted by name.
    pub listed: BTreeMap<PathBuf, Vec<Listed>>,
    /// What was read of each directory, to give the next walk.
    pub dirs: Dirs,
    /// How many directories were read: the others were as the last walk
    /// found them.
    pub relisted: usize,
}

/// An entry of the project: what a directory lists once ignored files are
/// out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    pub name: OsString,
    pub is_dir: bool,
}

/// The directories a walk entered, by project-relative path (the root is the
/// empty path).
pub type Dirs = BTreeMap<PathBuf, Dir>;

/// A directory as it was read: its `lstat` and everything it holds, ignored
/// or not. Sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dir {
    pub stat: Stat,
    pub entries: Vec<Raw>,
}

/// One entry of a directory as `readdir` gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Raw {
    pub name: OsString,
    pub kind: Kind,
}

/// What kind of entry a directory holds, as its listing says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    File,
    Dir,
    /// A symbolic link or anything else: never listed, never entered.
    Other,
}

/// What one `lstat` says of a file or a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Stat {
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub size: u64,
    pub inode: u64,
    pub mode: u32,
}

impl Stat {
    /// The stat of what `meta` describes.
    pub fn of(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let ns =
                |secs: i64, nanos: i64| secs.saturating_mul(1_000_000_000).saturating_add(nanos);
            Self {
                mtime_ns: ns(meta.mtime(), meta.mtime_nsec()),
                ctime_ns: ns(meta.ctime(), meta.ctime_nsec()),
                size: meta.size(),
                inode: meta.ino(),
                mode: meta.mode(),
            }
        }
        #[cfg(not(unix))]
        {
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos() as i64);
            Self {
                mtime_ns,
                ctime_ns: 0,
                size: meta.len(),
                inode: 0,
                mode: 0,
            }
        }
    }
}

impl ProjectFiles {
    /// Every file of the project and its `lstat`. `previous` is what the last
    /// walk read and `since` when that walk started, in nanoseconds since the
    /// epoch: without it every directory is read.
    pub fn walk(&self, previous: &Dirs, since: Option<i64>) -> Walked {
        let mut walk = Walk {
            project: self,
            root: self.root().to_owned(),
            previous,
            since,
            out: Walked::default(),
            files: Vec::new(),
        };
        walk.visit(Path::new(""), &mut Vec::new());
        walk.finish()
    }
}

impl Walk<'_> {
    fn visit(&mut self, rel: &Path, chain: &mut Vec<Level>) {
        let abs = self.root.join(rel);
        let stat = match fs::symlink_metadata(&abs) {
            Ok(meta) => Stat::of(&meta),
            Err(e) => {
                self.out
                    .listing
                    .errors
                    .push(format!("{}: {e}", abs.display()));
                return;
            }
        };
        let Some(dir) = self.read(rel, &abs, stat) else {
            return;
        };
        chain.push(Level::of(&abs, &dir.entries, &mut self.out.listing.errors));
        let mut listed = Vec::new();
        let mut below = Vec::new();
        for raw in &dir.entries {
            let at = rel.join(&raw.name);
            let is_dir = raw.kind == Kind::Dir;
            if !self.project.enters(&at, is_dir) || ignored(chain, &abs.join(&raw.name), is_dir) {
                continue;
            }
            match raw.kind {
                Kind::Dir => {
                    listed.push(Listed {
                        name: raw.name.clone(),
                        is_dir: true,
                    });
                    below.push(at);
                }
                Kind::File => {
                    listed.push(Listed {
                        name: raw.name.clone(),
                        is_dir: false,
                    });
                    self.files.push(at);
                }
                Kind::Other => {}
            }
        }
        self.out.listed.insert(rel.to_owned(), listed);
        self.out.dirs.insert(rel.to_owned(), dir);
        for at in below {
            self.visit(&at, chain);
        }
        chain.pop();
    }

    /// The directory as the last walk read it when its `lstat` is the same
    /// and not racy, else as `readdir` gives it now.
    fn read(&mut self, rel: &Path, abs: &Path, stat: Stat) -> Option<Dir> {
        let quiet = self
            .since
            .is_some_and(|since| stat.mtime_ns < since.saturating_sub(RACY_NS));
        if let Some(known) = self
            .previous
            .get(rel)
            .filter(|known| quiet && known.stat == stat)
        {
            return Some(known.clone());
        }
        let read = match fs::read_dir(abs) {
            Ok(read) => read,
            Err(e) => {
                self.out
                    .listing
                    .errors
                    .push(format!("{}: {e}", abs.display()));
                return None;
            }
        };
        self.out.relisted += 1;
        let mut entries = Vec::new();
        for entry in read {
            match entry {
                Ok(entry) => entries.push(Raw {
                    kind: match entry.file_type() {
                        Ok(kind) if kind.is_dir() => Kind::Dir,
                        Ok(kind) if kind.is_file() => Kind::File,
                        _ => Kind::Other,
                    },
                    name: entry.file_name(),
                }),
                Err(e) => self
                    .out
                    .listing
                    .errors
                    .push(format!("{}: {e}", abs.display())),
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Some(Dir { stat, entries })
    }

    fn finish(mut self) -> Walked {
        let root = self.root.clone();
        let stats = stat_all(&root, &self.files);
        let mut files: Vec<PathBuf> = Vec::with_capacity(stats.len());
        for (rel, stat) in stats {
            if let Some(stat) = stat {
                self.out.stats.insert(rel.clone(), stat);
                files.push(rel);
            }
        }
        files.sort();
        let (attributes, notices) = Attributes::new(&root, &files);
        let (kept, vendored): (Vec<PathBuf>, Vec<PathBuf>) =
            files.into_iter().partition(|f| !attributes.is_vendored(f));
        for gone in &vendored {
            self.out.stats.remove(gone);
        }
        let listed = &mut self.out.listed;
        let gone = |dir: &Path, name: &OsString| {
            let at = dir.join(name);
            vendored.binary_search(&at).is_ok() || !self.out.stats.contains_key(&at)
        };
        for (dir, children) in listed.iter_mut() {
            children.retain(|child| child.is_dir || !gone(dir, &child.name));
        }
        self.out.listing.files = kept;
        self.out.listing.notices = notices;
        self.out.listing.attributes = attributes;
        self.out
    }
}

/// The `lstat` of each file, on as many threads as the machine has: threads
/// of its own, since a caller may hold a lock that a work-stealing pool would
/// have it wait for from inside.
fn stat_all(root: &Path, files: &[PathBuf]) -> Vec<(PathBuf, Option<Stat>)> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = files.len().div_ceil(threads).max(256);
    let one = |rel: &PathBuf| {
        let stat = fs::symlink_metadata(root.join(rel))
            .ok()
            .map(|m| Stat::of(&m));
        (rel.clone(), stat)
    };
    std::thread::scope(|scope| {
        let jobs: Vec<_> = files
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(one).collect::<Vec<_>>()))
            .collect();
        jobs.into_iter()
            .flat_map(|job| job.join().expect("stat thread"))
            .collect()
    })
}
