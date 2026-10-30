//! Where the labels of a decision come from, and the labels.

use std::{env, path::PathBuf};

use lighthouse_ml::labels::{Judged, Sample, Labels, Table};
use lighthouse_spec::Decision;
use lighthouse_store::Records;

use crate::{Result, Session};

/// A repository the labels are about.
#[derive(Clone)]
pub struct Repo {
    /// The name the rows of the table of reviewed findings use (the `repo`
    /// column).
    pub name: Option<String>,
    pub dir: PathBuf,
}

impl Repo {
    /// The repository a `name=dir` names.
    pub fn parse(spec: &str) -> Result<Self> {
        let (name, dir) = spec
            .split_once('=')
            .ok_or_else(|| format!("`{spec}` is not name=dir"))?;
        Ok(Self {
            name: Some(name.to_owned()),
            dir: PathBuf::from(dir),
        })
    }
}

/// What the labels of a decision are drawn from.
#[derive(Clone, Default)]
pub struct Labelling {
    /// The repositories the labels are about; this project when empty.
    pub repos: Vec<Repo>,
    /// The table of reviewed findings, when there is one.
    pub table: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

impl Labelling {
    /// The session of each repository, in order. The first repository's root
    /// is the store of datasets and models of all of them: one model is
    /// trained on the labels of every repository, and each finds it there.
    pub(crate) fn sessions(&self) -> Result<Vec<(Repo, Session)>> {
        let start = env::current_dir()?;
        let repos = if self.repos.is_empty() {
            vec![Repo {
                name: None,
                dir: start,
            }]
        } else {
            let at = |repo: &Repo| Repo {
                name: repo.name.clone(),
                dir: start.join(&repo.dir),
            };
            self.repos.iter().map(at).collect()
        };
        let mut sessions: Vec<(Repo, Session)> = Vec::new();
        for repo in repos {
            let dir = repo
                .dir
                .canonicalize()
                .map_err(|e| format!("{}: {e}", repo.dir.display()))?;
            let session = Session::load_in(&dir, self.config.as_deref())?;
            let session = match sessions.first() {
                Some((_, first)) => session.stored_in(&first.store),
                None => session,
            };
            sessions.push((repo, session));
        }
        Ok(sessions)
    }

    /// The table of reviewed findings, restricted to the rows of `repo`.
    pub(crate) fn table_of<'a>(&'a self, repo: &'a Repo) -> Option<Table<'a>> {
        self.table.as_deref().map(|file| Table {
            file,
            repo: repo.name.as_deref(),
        })
    }
}

/// The labels of `decision` in the project of `session`: its judgments, the
/// rows of the table, and the `examples` the caller checked. A label
/// attributed to the model `exclude_model` is left out.
pub(crate) fn labels(
    session: &Session,
    decision: &Decision,
    table: Option<&Table>,
    exclude_model: Option<&str>,
    examples: Vec<Sample>,
) -> Result<Vec<Sample>> {
    let judgments = Records::read(&session.root)?
        .judgments
        .into_iter()
        .map(|event| event.spec)
        .filter(|spec| {
            spec.decision_uid
                .as_deref()
                .is_some_and(|uid| decision.uid() == Some(uid))
                || spec.decision == decision.id()
        })
        .map(|spec| Judged {
            fingerprint: spec.fingerprint,
            meaning_version: spec.meaning_version,
            judgment: spec.judgment,
            by: spec.was_attributed_to,
        })
        .collect();
    Labels {
        judgments,
        decision,
        table,
        exclude_model,
        examples,
    }
    .collect()
}
