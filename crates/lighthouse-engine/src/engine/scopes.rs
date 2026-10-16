use std::path::{Path, PathBuf};

use lighthouse_model::Incomplete;

use super::{Error, io_error};

/// The paths whose findings a run reports: project-relative files, or
/// directory prefixes; the empty path is the whole project.
#[derive(Debug, Default)]
pub(crate) struct Scopes(pub(crate) Vec<PathBuf>);

impl Scopes {
    /// The scopes of `paths`, relative to `root`, with the paths outside it as
    /// gaps in the analysis; no paths is the whole project.
    pub(crate) fn new(paths: &[PathBuf], root: &Path) -> Result<(Self, Vec<Incomplete>), Error> {
        if paths.is_empty() {
            return Ok((Self(vec![PathBuf::new()]), Vec::new()));
        }
        let (mut scopes, mut gaps) = (Vec::new(), Vec::new());
        for path in paths {
            let abs = path.canonicalize().map_err(io_error(path))?;
            match abs.strip_prefix(root) {
                Ok(rel) => scopes.push(rel.to_owned()),
                Err(_) => gaps.push(Incomplete {
                    path: Some(path.clone()),
                    reason: format!("outside {}, not checked", root.display()),
                }),
            }
        }
        Ok((Self(scopes), gaps))
    }

    /// Exactly these project-relative files; none means nothing is in scope.
    pub(crate) fn files(files: &[PathBuf]) -> Self {
        Self(files.to_vec())
    }

    /// Whether `path` lies in one of the scopes.
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.0.iter().any(|scope| path.starts_with(scope))
    }
}
