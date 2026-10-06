use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use lighthouse_spec::Catalog;

use crate::Result;

const PATTERNS_DIR: &str = "patterns";

pub fn generate(out: &Path) -> Result<u8> {
    let docs = lighthouse_spec::docs(Catalog::bundled());
    for orphan in orphans(out, &docs)? {
        fs::remove_file(&orphan)?;
        println!("removed {}", orphan.display());
    }
    for (path, text) in &docs {
        let target = out.join(path);
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&target, text)?;
        println!("wrote {}", target.display());
    }
    Ok(0)
}

pub fn check(out: &Path) -> Result<u8> {
    let docs = lighthouse_spec::docs(Catalog::bundled());
    let mut problems = Vec::new();
    for (path, text) in &docs {
        let target = out.join(path);
        match fs::read_to_string(&target) {
            Ok(current) if current == *text => {}
            Ok(_) => problems.push(format!("{} is stale", target.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                problems.push(format!("{} is missing", target.display()));
            }
            Err(e) => return Err(format!("cannot read {}: {e}", target.display()).into()),
        }
    }
    for orphan in orphans(out, &docs)? {
        problems.push(format!("{} is not generated", orphan.display()));
    }
    for problem in &problems {
        eprintln!("lighthouse: {problem}");
    }
    if !problems.is_empty() {
        eprintln!("lighthouse: run `lighthouse docs generate` to refresh");
    }
    Ok(u8::from(!problems.is_empty()))
}

/// Files in the generated directory that no pack produces.
fn orphans(out: &Path, docs: &BTreeMap<String, String>) -> Result<Vec<PathBuf>> {
    let dir = out.join(PATTERNS_DIR);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", dir.display()).into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !docs.contains_key(&format!("{PATTERNS_DIR}/{name}")) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}
