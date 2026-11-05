//! The walk of a project: the listing [`ProjectFiles`] gives, with the `lstat`
//! of every file.
//!
//! The rules are the `ignore` crate walker's, which this is tested against
//! ([`ProjectFiles::walker_list`]): `.gitignore` files from the root down
//! (the deepest match wins), the `info/exclude` of each repository found,
//! then [`ProjectFiles::enters`] and `.gitattributes`. What a directory holds
//! and the text of the ignore files come from a [`Source`], so that a caller
//! with a cache (the engine's graph) reads a directory again only when it
//! changed.

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

/// How close to the start of a walk a modification time is for the `lstat` not
/// to be trusted: see [`Stat::racy`].
pub const RACY_NS: i64 = 2_000_000_000;

/// Where a walk gets what a directory holds and the text of the ignore files.
pub trait Source {
    /// What the directory at the project-relative `rel`, whose `lstat` is
    /// `stat`, holds.
    fn entries(&mut self, rel: &Path, stat: Stat) -> Entries;

    /// The text of the ignore file at the project-relative `rel`, if it can
    /// be read as text.
    fn text(&mut self, rel: &Path) -> Option<String>;
}

/// The project on disk: every directory is read, every file opened.
pub struct Disk<'a>(pub &'a Path);

impl Source for Disk<'_> {
    fn entries(&mut self, rel: &Path, _: Stat) -> Entries {
        read_entries(&self.0.join(rel))
    }

    fn text(&mut self, rel: &Path) -> Option<String> {
        fs::read_to_string(self.0.join(rel)).ok()
    }
}

struct Walk<'a, S> {
    project: &'a ProjectFiles,
    root: PathBuf,
    source: &'a mut S,
    out: Walked,
    files: Vec<PathBuf>,
}

/// What a walk found.
#[derive(Default)]
pub struct Walked {
    pub listing: Listing,
    /// The `lstat` of each listed file.
    pub stats: BTreeMap<PathBuf, Stat>,
}

/// What a directory holds, everything, ignored or not, sorted by name, and
/// what could not be read of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entries {
    pub raw: Vec<Raw>,
    pub errors: Vec<String>,
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
    /// Whether a change after this `lstat` may not show in it: the file
    /// system's clock may be as coarse as two seconds (FAT), so one written
    /// within [`RACY_NS`] of the walk that started at `started` (nanoseconds
    /// since the epoch) is not trusted yet. Git's racy rule.
    pub fn racy(&self, started: i64) -> bool {
        self.mtime_ns >= started.saturating_sub(RACY_NS)
    }

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

impl<S: Source> Walk<'_, S> {
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
        let entries = self.source.entries(rel, stat);
        let unread = entries.raw.is_empty() && !entries.errors.is_empty();
        self.out.listing.errors.extend(entries.errors);
        if unread {
            return;
        }
        let level = Level::of(
            rel,
            &abs,
            &entries.raw,
            self.source,
            &mut self.out.listing.errors,
        );
        chain.push(level);
        let mut below = Vec::new();
        for raw in &entries.raw {
            let at = rel.join(&raw.name);
            let is_dir = raw.kind == Kind::Dir;
            if !self.project.enters(&at, is_dir) || ignored(chain, &abs.join(&raw.name), is_dir) {
                continue;
            }
            match raw.kind {
                Kind::Dir => below.push(at),
                Kind::File => self.files.push(at),
                Kind::Other => {}
            }
        }
        for at in below {
            self.visit(&at, chain);
        }
        chain.pop();
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
        let texts = files
            .iter()
            .filter(|f| Attributes::is_file(f))
            .map(|f| (f.clone(), self.source.text(f)))
            .collect();
        let (attributes, notices) = Attributes::from_texts(texts);
        let (kept, vendored): (Vec<PathBuf>, Vec<PathBuf>) =
            files.into_iter().partition(|f| !attributes.is_vendored(f));
        for gone in &vendored {
            self.out.stats.remove(gone);
        }
        self.out.listing.files = kept;
        self.out.listing.notices = notices;
        self.out.listing.attributes = attributes;
        self.out
    }
}

/// The walk of `project` over what `source` holds: [`ProjectFiles::walk`].
pub(crate) fn walk(project: &ProjectFiles, source: &mut impl Source) -> Walked {
    let mut walk = Walk {
        project,
        root: project.root().to_owned(),
        source,
        out: Walked::default(),
        files: Vec::new(),
    };
    walk.visit(Path::new(""), &mut Vec::new());
    walk.finish()
}

/// Reads the directory at `abs`.
pub fn read_entries(abs: &Path) -> Entries {
    let mut entries = Entries::default();
    let read = match fs::read_dir(abs) {
        Ok(read) => read,
        Err(e) => {
            entries.errors.push(format!("{}: {e}", abs.display()));
            return entries;
        }
    };
    for entry in read {
        match entry {
            Ok(entry) => entries.raw.push(Raw {
                kind: match entry.file_type() {
                    Ok(kind) if kind.is_dir() => Kind::Dir,
                    Ok(kind) if kind.is_file() => Kind::File,
                    _ => Kind::Other,
                },
                name: entry.file_name(),
            }),
            Err(e) => entries.errors.push(format!("{}: {e}", abs.display())),
        }
    }
    entries.raw.sort_by(|a, b| a.name.cmp(&b.name));
    entries
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
