//! The fragment cache: the symbols, edges and summaries of a file are kept
//! between runs, keyed by the file's text and by a digest of everything the
//! other files and manifests contribute to its analysis. A body edit changes
//! the text of one file and no one's contribution (function bodies are left
//! out of it), so only that file is analyzed again. Any other change misses
//! everything. The module tree and name resolution always run: they are the
//! cheap part and produce the context the digest stands for.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use lighthouse_protocol::{Fragment, IndexParams};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use syn::{ImplItem, Item, TraitItem};

use crate::{
    extract::MacroStats,
    tree::{SourceFile, Tree},
};

/// Versions the layout and meaning of the cache file.
const SCHEMA: &str = "lang-rust-cache/1";
const FILE: &str = "fragments.json";
const LANGUAGE: &str = "rust";

/// Makes the name of each temporary file unique within the process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What the cache keeps of one analyzed file.
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    key: String,
    fragment: Fragment,
    defines: bool,
    unread: u32,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    version: String,
    files: BTreeMap<String, Entry>,
}

/// The cache of one index request.
pub struct Cache {
    path: PathBuf,
    version: String,
    context: Option<String>,
    stored: BTreeMap<String, Entry>,
    fresh: BTreeMap<String, Entry>,
    analyzed: usize,
    /// How many files the cache file held when it was read.
    opened: usize,
}

impl Cache {
    /// The cache for a request, none when the host gave no directory or sent
    /// overlays, whose text is not on disk.
    pub fn open(
        params: &IndexParams,
        provider: &str,
        tree: &Tree,
        files: &[String],
    ) -> Option<Self> {
        let dir = &params.context.cache.as_ref()?.dir;
        if params
            .context
            .overlays
            .as_ref()
            .is_some_and(|o| !o.is_empty())
        {
            return None;
        }
        let version = digest(&[SCHEMA, provider, &executable_digest()]);
        let path = Path::new(dir).join(FILE);
        let stored = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .filter(|s| s.version == version)
            .map(|s| s.files)
            .unwrap_or_default();
        let opened = stored.len();
        let mut cache = Self {
            path,
            version,
            context: None,
            stored,
            fresh: BTreeMap::new(),
            analyzed: 0,
            opened,
        };
        cache.bind(tree, params, files);
        Some(cache)
    }

    /// Fixes the context of the run: the options, the manifests above the
    /// requested files, and the shape of every source file the module tree
    /// reached. A tree with a problem has no context, so nothing is cached.
    fn bind(&mut self, tree: &Tree, params: &IndexParams, files: &[String]) {
        if !tree.problems.is_empty() {
            return;
        }
        let options = params
            .context
            .options
            .get(LANGUAGE)
            .map_or_else(String::new, ToString::to_string);
        let root = crate::provider::normalize(Path::new(&params.project.root));
        let mut parts = vec![self.version.clone(), options];
        for manifest in manifests(&root, files) {
            parts.push(manifest);
        }
        let mut shapes: Vec<(&str, String)> = tree
            .files
            .iter()
            .map(|f| (f.rel.as_str(), skeleton(f)))
            .collect();
        shapes.sort();
        for (rel, shape) in shapes {
            parts.push(rel.to_owned());
            parts.push(shape);
        }
        self.context = Some(digest(
            &parts.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
    }

    /// The stored analysis of `file` if its text and context are unchanged.
    pub fn get(&mut self, file: &SourceFile) -> Option<(Fragment, MacroStats)> {
        let key = self.key(file)?;
        if self.stored.get(&file.rel)?.key != key {
            return None;
        }
        let entry = self.stored.remove(&file.rel)?;
        let stats = MacroStats {
            defines: entry.defines,
            unread: entry.unread,
        };
        let fragment = entry.fragment.clone();
        self.fresh.insert(file.rel.clone(), entry);
        Some((fragment, stats))
    }

    /// Remembers the analysis of `file`.
    pub fn put(&mut self, file: &SourceFile, fragment: &Fragment, stats: &MacroStats) {
        self.analyzed += 1;
        let Some(key) = self.key(file) else {
            return;
        };
        self.fresh.insert(
            file.rel.clone(),
            Entry {
                key,
                fragment: fragment.clone(),
                defines: stats.defines,
                unread: stats.unread,
            },
        );
    }

    fn key(&self, file: &SourceFile) -> Option<String> {
        let context = self.context.as_ref()?;
        Some(digest(&[context, &file.rel, &digest(&[&file.text])]))
    }

    /// Writes the cache when the run changed it. A failure is returned as a
    /// notice; the results of the run do not depend on it.
    pub fn save(self) -> Option<String> {
        self.context.as_ref()?;
        let unchanged = self.analyzed == 0 && self.fresh.len() == self.opened;
        if unchanged {
            return None;
        }
        let stored = Stored {
            version: self.version,
            files: self.fresh,
        };
        write_atomic(&self.path, &stored).err().map(|e| {
            format!(
                "cache file {} is not writable, results are not cached: {e}",
                self.path.display()
            )
        })
    }
}

fn write_atomic(path: &Path, stored: &Stored) -> Result<(), String> {
    let dir = path.parent().ok_or("no directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec(stored).map_err(|e| e.to_string())?;
    fs::write(&tmp, bytes)
        .and_then(|()| fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
        .map_err(|e| e.to_string())
}

/// The digest of every `Cargo.toml` at or above the directories of the files,
/// above the root too (`publish.workspace` reads the workspace manifest there):
/// what decides packages, targets and dependencies.
fn manifests(root: &Path, files: &[String]) -> Vec<String> {
    let dirs: BTreeSet<PathBuf> = files
        .iter()
        .filter_map(|rel| root.join(rel).parent().map(Path::to_owned))
        .flat_map(|dir| dir.ancestors().map(Path::to_owned).collect::<Vec<_>>())
        .collect();
    dirs.iter()
        .filter_map(|dir| {
            let text = fs::read(dir.join("Cargo.toml")).ok()?;
            Some(digest(&[
                &dir.to_string_lossy(),
                &String::from_utf8_lossy(&text),
            ]))
        })
        .collect()
}

/// The digest of a file without the bodies of its functions: what the rest of
/// the project can see of it.
fn skeleton(file: &SourceFile) -> String {
    let mut bodies = Vec::new();
    collect_bodies(file, &file.ast.items, &mut bodies);
    let mut kept = String::with_capacity(file.text.len());
    let mut at = 0;
    for (start, end) in bodies {
        if start < at
            || end > file.text.len()
            || !file.text.is_char_boundary(start)
            || !file.text.is_char_boundary(end)
        {
            return digest(&[&file.text]);
        }
        kept.push_str(&file.text[at..start]);
        at = end;
    }
    kept.push_str(&file.text[at..]);
    digest(&[&kept])
}

fn collect_bodies(file: &SourceFile, items: &[Item], out: &mut Vec<(usize, usize)>) {
    for item in items {
        match item {
            Item::Fn(f) => out.extend(span_of(file, &f.block)),
            Item::Impl(i) => {
                for member in &i.items {
                    if let ImplItem::Fn(f) = member {
                        out.extend(span_of(file, &f.block));
                    }
                }
            }
            Item::Trait(t) => {
                for member in &t.items {
                    if let TraitItem::Fn(f) = member
                        && let Some(block) = &f.default
                    {
                        out.extend(span_of(file, block));
                    }
                }
            }
            Item::Mod(m) => {
                if let Some((_, nested)) = &m.content {
                    collect_bodies(file, nested, out);
                }
            }
            _ => {}
        }
    }
}

/// The byte range of a block, braces included.
fn span_of(file: &SourceFile, block: &syn::Block) -> Option<(usize, usize)> {
    let open = block.brace_token.span.open().start();
    let close = block.brace_token.span.close().end();
    Some((
        file.offset(open.line, open.column)?,
        file.offset(close.line, close.column)?,
    ))
}

/// The digest of the running binary: a rebuilt provider must not read what an
/// older one wrote.
fn executable_digest() -> String {
    std::env::current_exe().and_then(fs::read).map_or_else(
        |_| "unknown".to_owned(),
        |bytes| hex(&Sha256::digest(bytes)),
    )
}

fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_string().as_bytes());
        hasher.update(b":");
        hasher.update(part.as_bytes());
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
