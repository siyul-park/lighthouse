use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use lighthouse_session::{Session, skill_for};
use lighthouse_spec::Catalog;

use crate::Result;

const DECISIONS_DIR: &str = lighthouse_spec::DOCS_DIR;

pub fn generate(out: &Path, skill: &Path) -> Result<u8> {
    let docs = generated();
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
    if let Some(dir) = skill.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(skill, skill_text()?)?;
    println!("wrote {}", skill.display());
    Ok(0)
}

/// Every generated page: one per pack, and the fix operations with the order
/// keys the bundled plugins register.
fn generated() -> BTreeMap<String, String> {
    let mut docs = lighthouse_spec::docs(Catalog::bundled());
    let keys: Vec<(String, String)> = lighthouse_checks::registry()
        .order_keys()
        .map(|k| {
            let manifest = k.manifest();
            (manifest.id.clone(), manifest.description.clone())
        })
        .collect();
    docs.insert(
        format!("{DECISIONS_DIR}/fix-operations.md"),
        lighthouse_spec::fix_operations_markdown(&keys),
    );
    docs
}

/// The agent skill of this project: its catalog and configuration.
fn skill_text() -> Result<String> {
    skill_for(&Session::load_or_default(None)?)
}

pub fn check(out: &Path, skill: &Path) -> Result<u8> {
    let docs = generated();
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
    match fs::read_to_string(skill) {
        Ok(current) if current == skill_text()? => {}
        Ok(_) => problems.push(format!("{} is stale", skill.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            problems.push(format!("{} is missing", skill.display()));
        }
        Err(e) => return Err(format!("cannot read {}: {e}", skill.display()).into()),
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
    let dir = out.join(DECISIONS_DIR);
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
        if !docs.contains_key(&format!("{DECISIONS_DIR}/{name}")) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}
