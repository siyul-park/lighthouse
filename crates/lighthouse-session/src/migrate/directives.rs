//! Rewrites the first form of the suppression directive, `lighthouse:allow`,
//! as its ESLint-style successor `lighthouse-disable-next-line` in source
//! files, and the ids of renamed decisions in every directive line. Only a comment line that starts with the marker is touched, as
//! the engine reads it; prose that mentions the marker, strings, and the
//! structured documents of Lighthouse (YAML, TOML, JSON) are left as they are.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{Action, Plan, file_name, walk};
use crate::Result;

const OLD: &str = "lighthouse:allow";
/// The markers of a directive, longest first.
const MARKERS: [&str; 5] = [
    "lighthouse-disable-next-line",
    "lighthouse-disable-line",
    "lighthouse-disable",
    "lighthouse-enable",
    OLD,
];
const NEW: &str = "lighthouse-disable-next-line";
/// Files that hold prose or structured documents, not source code.
const SKIPPED_EXTENSIONS: [&str; 10] = [
    "md", "markdown", "rst", "txt", "adoc", "yaml", "yml", "toml", "json", "lock",
];

/// Plans the rewrite of every source file under `paths`.
pub(super) fn rewrite(paths: &[PathBuf], plan: &mut Plan) -> Result<()> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, &mut files)?;
        } else if path.is_file() {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    for path in files.into_iter().filter(|f| source(f)) {
        if plan.actions.contains_key(&path) {
            continue;
        }
        // A file that is not UTF-8 text is not source we can edit.
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(text) = migrated(&text) {
            plan.actions.insert(path, Action::Write(text));
        }
    }
    Ok(())
}

fn source(path: &Path) -> bool {
    let name = file_name(path);
    let extension = name.rsplit_once('.').map_or("", |(_, e)| e);
    !name.starts_with('.') && !SKIPPED_EXTENSIONS.contains(&extension)
}

/// `text` with every directive line migrated; `None` when none is.
fn migrated(text: &str) -> Option<String> {
    if !text.contains(OLD) && !text.contains("lighthouse-") {
        return None;
    }
    let mut changed = false;
    let out: String = text
        .split_inclusive('\n')
        .map(|line| match migrated_line(line) {
            Some(line) => {
                changed = true;
                line
            }
            None => line.to_owned(),
        })
        .collect();
    changed.then_some(out)
}

/// The line with the marker replaced, when the line is a comment that starts
/// with it.
fn migrated_line(line: &str) -> Option<String> {
    let body = line.trim_start_matches(|c: char| c.is_whitespace() || "/*#!".contains(c));
    let prefix = &line[..line.len() - body.len()];
    if !prefix.contains(['/', '*', '#']) {
        return None;
    }
    let (marker, after) = MARKERS.iter().find_map(|marker| {
        let after = body.strip_prefix(marker)?;
        (after.is_empty() || after.starts_with(char::is_whitespace)).then_some((*marker, after))
    })?;
    let marker = if marker == OLD { NEW } else { marker };
    let renamed = renamed_ids(after);
    let migrated = format!("{prefix}{marker}{renamed}");
    (migrated != line).then_some(migrated)
}

/// The ids of the directive, up to its ` -- ` reason, by the names they have now.
fn renamed_ids(after: &str) -> String {
    let aliases = lighthouse_spec::Catalog::bundled().aliases();
    let (ids, reason) = after.split_once(" --").map_or((after, ""), |(i, r)| (i, r));
    let ids: Vec<String> = ids
        .split(',')
        .map(|id| {
            let name = id.trim();
            match aliases.get(name) {
                Some(new) => id.replacen(name, new, 1),
                None => id.to_owned(),
            }
        })
        .collect();
    let rest = if reason.is_empty() && !after.contains(" --") {
        String::new()
    } else {
        format!(" --{reason}")
    };
    format!("{}{rest}", ids.join(","))
}
