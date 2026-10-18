//! `lighthouse eval`: runs the checks of a decision, and of a candidate beside
//! it, on the repositories its labels come from, and hands what they found to
//! the host's evaluation (`lighthouse_ml::evaluate`). The model a revision
//! attached, and the one `--model` names, are layers laid over the run of the
//! check without models attached.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use lighthouse_engine::ModelOptions;
use lighthouse_ml::{
    evaluate::{Candidate, EvalReport, Found, Overlaid, Overlay, RepoResult, grade},
    labels::{Label, Source},
};
use lighthouse_model::Severity;
use lighthouse_spec::{Catalog, local_files};

use super::{Labelling, Repo, attached, examples, labelling};
use crate::{CheckRequest, Checked, Result, Session, check};

/// What to evaluate.
pub struct EvalRequest {
    pub decision: String,
    pub labelling: Labelling,
    /// A decision file whose check is evaluated beside the current one.
    pub candidate: Option<PathBuf>,
    /// A model (`<provider>/<model>@sha256:<hex>`) to grade as the first
    /// layer of the decision, beside the current check.
    pub model: Option<String>,
    /// Write this many selected subjects the check did not flag to a table
    /// a person labels, to measure recall.
    pub audit: Option<usize>,
}

/// A candidate decision file to run beside the project's decisions.
struct CandidateFile {
    /// The id of the candidate decision.
    id: String,
    /// The project's own decisions with the candidate file added.
    local: Catalog,
}

impl CandidateFile {
    fn read(root: &Path, file: &Path) -> Result<Self> {
        let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let name = file.file_name().map_or_else(
            || "candidate.yaml".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let id = Catalog::from_local(BTreeMap::from([(name.clone(), text.clone())]))?
            .decisions()
            .next()
            .map(|d| d.id().to_owned())
            .ok_or("the candidate file holds no decision")?;
        let mut files = local_files(root)?.unwrap_or_default();
        files.insert(name, text);
        Ok(Self {
            id,
            local: Catalog::from_local(files)?,
        })
    }

    /// The project with the candidate's decisions on top and its rule enabled.
    fn session(&self, session: &Session) -> Result<Session> {
        let mut session = session.with_local(Some(self.local.clone()))?;
        session.config = session.config.with_rule(&self.id, Severity::Warn);
        Ok(session)
    }
}

/// Runs the evaluation.
pub fn eval(request: &EvalRequest) -> Result<EvalReport> {
    if request.model.is_some() && request.candidate.is_some() {
        return Err("--model and --candidate are exclusive".into());
    }
    let results = request
        .labelling
        .sessions()?
        .iter()
        .enumerate()
        // The examples are the decision's, not the repository's.
        .map(|(at, (repo, session))| one(request, repo, session, at == 0))
        .collect::<Result<Vec<_>>>()?;
    let rows: Vec<String> = results.iter().flat_map(|r| r.audit.clone()).collect();
    let mut report = EvalReport::pool(results)?;
    if request.audit.is_some() && !rows.is_empty() {
        let path = PathBuf::from(format!("{}.audit.tsv", request.decision.replace('/', "-")));
        let mut text = String::from("fingerprint\trepo\tlocation\tverdict\treason\n");
        for row in rows {
            text.push_str(&row);
            text.push('\n');
        }
        fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        report.audit = Some(path);
    }
    Ok(report)
}

/// What the repository of `session` gives: its labels graded by the check of
/// the decision and, when there is one, by the candidate or the model beside
/// it. The examples are subjects in the first repository only.
fn one(request: &EvalRequest, repo: &Repo, session: &Session, first: bool) -> Result<RepoResult> {
    let catalog = &session.catalog;
    let decision = catalog
        .decision(&request.decision)
        .ok_or_else(|| format!("unknown decision `{}`", request.decision))?;
    let policy = session
        .config
        .models(catalog.projects())?
        .policy(&request.decision);
    // The base run leaves the layer a revision attaches out: it is laid over
    // the run afterwards, by the same code that grades a model not attached.
    let models = ModelOptions {
        all: true,
        ..ModelOptions::default()
    };
    let examples = if first {
        examples::labels(session, decision, decision.id(), &models)?
    } else {
        Vec::new()
    };
    // Only the labels a person gave, a reviewed table holds or an example
    // shows grade the check; weak labels (an agent's, a model's) never do.
    let table = request.labelling.table_of(repo);
    let labels: Vec<Label> = labelling::labels(session, decision, table.as_ref(), None, examples)?
        .into_iter()
        .filter(|l| l.source.weight() >= Source::Person.weight())
        .collect();
    let attached = attached(&session.root, catalog).remove(&request.decision);
    let layered = attached.is_some() || request.model.is_some();
    // Models are asked about the labelled subjects only; a label with an
    // observed outcome (an example) is graded without a run.
    let labelled = labels
        .iter()
        .filter(|l| l.observed.is_none())
        .map(|l| l.key.clone())
        .collect();
    let (checked, base) = run(
        session,
        &request.decision,
        Some(labelled),
        layered || request.audit.is_some(),
    )?;
    let reference = decision.reference();
    let models_of = |model: &str| Ok(checked.engine.model(model)?);
    let layer = |model| Overlay {
        model,
        decision: &reference,
        root: &session.store,
        models: &models_of,
    };
    let current = attached
        .as_deref()
        .map(|model| layer(model).apply(&base, &labels))
        .transpose()?;
    let (current, labels) = current
        .as_ref()
        .map_or((&base, &labels), |l| (&l.found, &l.labels));
    let beside = if let Some(model) = request.model.as_deref() {
        let Overlaid {
            found,
            labels: shown,
        } = layer(model).apply(&base, labels)?;
        Some((request.decision.clone(), found, shown))
    } else if let Some(file) = request.candidate.as_deref() {
        let file = CandidateFile::read(&session.root, file)?;
        let beside = file.session(session)?;
        // The candidate's fingerprints are not known before it runs, so it
        // asks models about everything it selects.
        let (_, found) = run(&beside, &file.id, None, request.audit.is_some())?;
        let shown = if first {
            examples::labels(&beside, decision, &file.id, &models)?
        } else {
            Vec::new()
        };
        Some((file.id, found, shown))
    } else {
        None
    };
    let candidate = beside.as_ref().map(|(id, found, shown)| Candidate {
        id: id.clone(),
        found,
        labels: shown,
    });
    Ok(grade(
        &request.decision,
        request.audit,
        repo.name.as_deref(),
        policy,
        labels,
        current,
        candidate,
    ))
}

/// Runs the check of `rule` without the models revisions attached, asking
/// models about the subjects with these `fingerprints` (all it selects when
/// none), and keeping the subjects they are put when `collect`.
fn run(
    session: &Session,
    rule: &str,
    fingerprints: Option<BTreeSet<String>>,
    collect: bool,
) -> Result<(Checked, Found)> {
    let checked = check(
        session,
        &CheckRequest {
            rules: vec![rule.to_owned()],
            store: false,
            cache: false,
            detached: true,
            models: ModelOptions {
                all: true,
                fingerprints,
                collect,
                ..ModelOptions::default()
            },
            ..CheckRequest::default()
        },
    )?;
    let found = Found::of(&checked.run);
    Ok((checked, found))
}
