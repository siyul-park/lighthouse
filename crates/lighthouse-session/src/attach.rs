//! `lighthouse model attach` and `model detach`: the revisions that bind a
//! model to a decision and unbind it. Attaching evaluates the model first and
//! writes nothing unless the evaluation allows it.

use std::path::Path;

use lighthouse_ml::attach::{attachable, evidence};
use lighthouse_model::{Attribution, now};
use lighthouse_spec::Decision;
use lighthouse_store::{Change, Records, RevisionSpec, append_revision};
use serde_json::{Value, json};

use crate::{EvalReport, EvalRequest, Result, Session, eval};

/// What came of asking to attach a model.
pub struct Attachment {
    pub decision: String,
    pub model: String,
    /// The revision that attached it; none when it was refused.
    pub revision: Option<String>,
    /// Why the evaluation does not allow it.
    pub reasons: Vec<String>,
    /// The numbers the gate looked at, as a revision records them.
    pub evaluation: Value,
    pub report: Box<EvalReport>,
}

/// The revision that detached a model.
pub struct Detachment {
    pub revision: String,
    pub decision: String,
    pub model: String,
}

/// A minor revision of a decision, on top of its latest.
struct Revision<'a> {
    records: &'a Records,
    decision: &'a Decision,
    /// Who approves it.
    approval: Attribution,
    /// The model it binds; none unbinds.
    model: Option<&'a str>,
    /// What justifies it.
    evaluation: Value,
}

/// Evaluates `request.model` as the first layer of the decision and, when the
/// evaluation allows it, appends the revision that attaches it.
pub fn attach(request: &EvalRequest, approval: Attribution) -> Result<Attachment> {
    let model = request
        .model
        .clone()
        .ok_or("--model names the model to attach")?;
    let session = Session::load(request.config.as_deref())?;
    let catalog = session.catalog()?;
    let decision = catalog
        .decision(&request.decision)
        .ok_or_else(|| format!("unknown decision `{}`", request.decision))?;
    let records = Records::read(&session.root)?;
    let report = eval(request)?;
    let evaluation = evidence(&report, &model, &session.root)?;
    let mut reasons = attachable(&report);
    if records.attached().get(key_of(decision)) == Some(&model) {
        reasons.push(format!("`{model}` is already attached"));
    }
    let revision = if reasons.is_empty() {
        let revision = Revision {
            records: &records,
            decision,
            approval,
            model: Some(&model),
            evaluation: evaluation.clone(),
        };
        Some(append(&session.root, revision)?)
    } else {
        None
    };
    Ok(Attachment {
        decision: decision.id().to_owned(),
        model,
        revision,
        reasons,
        evaluation,
        report: Box::new(report),
    })
}

/// Appends the revision that unbinds the model of `decision`.
pub fn detach(config: Option<&Path>, decision: &str, approval: Attribution) -> Result<Detachment> {
    let session = Session::load(config)?;
    let catalog = session.catalog()?;
    let found = catalog
        .decision(decision)
        .ok_or_else(|| format!("unknown decision `{decision}`"))?;
    let records = Records::read(&session.root)?;
    let model = records
        .attached()
        .remove(key_of(found))
        .ok_or_else(|| format!("no model is attached to `{decision}`"))?;
    let revision = append(
        &session.root,
        Revision {
            records: &records,
            decision: found,
            approval,
            model: None,
            evaluation: json!({ "detached": model }),
        },
    )?;
    Ok(Detachment {
        revision,
        decision: found.id().to_owned(),
        model,
    })
}

/// How the log names a decision: its uid, else its id.
pub(crate) fn key_of(decision: &Decision) -> &str {
    decision.uid().unwrap_or_else(|| decision.id())
}

/// Appends `revision` to the log and gives its id.
fn append(root: &Path, revision: Revision) -> Result<String> {
    let decision = revision.decision;
    let key = key_of(decision);
    Ok(append_revision(
        root,
        RevisionSpec {
            decision: key.to_owned(),
            parent: revision.records.latest(key).map(|latest| latest.id.clone()),
            meaning_version: Some(decision.meaning_version()),
            check_revision: Some(decision.check_revision()),
            change: Change::Minor,
            evaluation: revision.evaluation,
            approval: revision.approval,
            model: revision.model.map(str::to_owned),
            generated_at_time: now(),
        },
    )?)
}
