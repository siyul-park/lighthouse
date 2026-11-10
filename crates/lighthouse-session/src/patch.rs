//! A unified diff as the files it would write: what `decision ask --edit`
//! takes. The diff is git's (`git diff`, `git apply` reads it), with paths
//! relative to the project root; each file of it is applied in memory.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use diffy::Patch;
use lighthouse_engine::Overlays;
use lighthouse_spec::FILE_NAMES;

use crate::Result;

/// The null side of a diff that creates or deletes a file.
const NULL: &str = "/dev/null";

/// What a git diff says besides hunks that this reading refuses.
const REFUSED: [(&str, &str); 7] = [
    ("rename from ", "a rename"),
    ("copy from ", "a copy"),
    ("old mode ", "a mode change"),
    ("deleted file mode ", "a deletion"),
    ("+++ /dev/null", "a deletion"),
    ("Binary files ", "a binary file"),
    ("GIT binary patch", "a binary file"),
];

/// The texts the files of `diff` would have, by project-relative path. Fails,
/// naming the file, for a diff that is not a plain edit of text files, or a
/// hunk that does not apply to the file as it is under `root`.
pub fn overlays(root: &Path, diff: &str) -> Result<Overlays> {
    let files = sections(diff)
        .iter()
        .map(|section| apply(root, section))
        .collect::<Result<Vec<_>>>()?;
    let count = files.len();
    let out: Overlays = files.into_iter().collect();
    match out.len() {
        0 => Err("the diff has no file in it".into()),
        n if n < count => Err("the diff edits a file twice".into()),
        _ => Ok(out),
    }
}

/// The diff cut into one piece per file: at each `diff --git` line, or when
/// there are none at each `--- ` line that a `+++ ` line and a hunk header
/// follow (or a `Binary files` line). What comes before the first (a mail's
/// headers, for one) is no file; nor the signature a mail ends with.
fn sections(diff: &str) -> Vec<String> {
    let git = diff.lines().any(|line| line.starts_with("diff --git "));
    let mut lines: Vec<&str> = diff.split_inclusive('\n').collect();
    // A mail ends with a signature: `-- ` and the version of git.
    if lines.len() >= 2 && lines[lines.len() - 2] == "-- \n" {
        lines.truncate(lines.len() - 2);
    }
    let mut out: Vec<String> = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let follows =
            |n: usize, prefix: &str| lines.get(at + n).is_some_and(|l| l.starts_with(prefix));
        let starts = if git {
            line.starts_with("diff --git ")
        } else {
            line.starts_with("Binary files ")
                || line.starts_with("--- ") && follows(1, "+++ ") && follows(2, "@@ ")
        };
        if starts {
            out.push(String::new());
        }
        if let Some(last) = out.last_mut() {
            last.push_str(line);
        }
    }
    out
}

/// The path of one file's section and the text it gives the file.
fn apply(root: &Path, section: &str) -> Result<(PathBuf, String)> {
    let refused = REFUSED
        .iter()
        .find(|(marker, _)| section.lines().any(|line| line.starts_with(marker)));
    if let Some((_, what)) = refused {
        return Err(format!("the diff has {what}: only edits and new files are taken").into());
    }
    if !section.lines().any(|line| line.starts_with("@@ ")) {
        return Err("a file of the diff has no hunk: an empty new file is not taken".into());
    }
    let patch = Patch::from_str(section).map_err(|e| e.to_string())?;
    let (old, new) = (patch.original(), patch.modified());
    let created = old == Some(NULL);
    let path = relative(new.or(old).ok_or("a file of the diff has no name")?)?;
    refuse(&path)?;
    let before = if created {
        let parent_is_there = path.parent().is_some_and(|dir| root.join(dir).is_dir());
        if root.join(&path).exists() || !parent_is_there {
            return Err(format!(
                "{}: a new file is one that does not exist, in a directory the project has",
                path.display()
            )
            .into());
        }
        String::new()
    } else {
        fs::read_to_string(root.join(&path)).map_err(|e| format!("{}: {e}", path.display()))?
    };
    let after = diffy::apply(&before, &patch)
        .map_err(|e| format!("{}: {e} does not apply", path.display()))?;
    Ok((path, after))
}

/// The project-relative path a diff names: git's `a/` and `b/` prefix
/// stripped, and nothing that leaves the project.
fn relative(name: &str) -> Result<PathBuf> {
    let name = name.split('\t').next().unwrap_or(name);
    let name = name
        .strip_prefix("a/")
        .or_else(|| name.strip_prefix("b/"))
        .unwrap_or(name);
    let path = PathBuf::from(name);
    let plain = path
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    if !plain {
        return Err(format!("`{name}` is not a path under the project root").into());
    }
    Ok(path)
}

/// Fails for a file that makes the project's configuration or its decisions,
/// which an overlay would change under the run.
fn refuse(path: &Path) -> Result<()> {
    let configures = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| FILE_NAMES.contains(&name));
    if configures || path.starts_with(".lighthouse") {
        return Err(format!(
            "{}: the configuration and the decisions cannot be edited by a what-if",
            path.display()
        )
        .into());
    }
    Ok(())
}
