//! `lighthouse model attach` and `model detach`: the revisions that bind a
//! model to a decision and unbind it. Attaching evaluates the model first and
//! writes nothing unless the evaluation allows it.

use std::path::Path;

use lighthouse_ml::attach::{attachable, evidence};
use lighthouse_model::{Attribution, now};
use lighthouse_spec::Decision;
use lighthouse_store::{Change, Records, RevisionSpec, append_revision};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use super::{EvalReport, EvalRequest, eval};
use crate::{Result, Session};

/// What came of asking to attach a model: the `ModelAttach` kind.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Attachment {
    pub decision: String,
    pub model: String,
    /// The revision that attached it; none when it was refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Why the evaluation does not allow it; none when it was attached.
    #[serde(rename = "refused", skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// The numbers the gate looked at, as a revision records them.
    pub evaluation: Value,
    #[serde(skip)]
    pub report: EvalReport,
}

/// The revision that detached a model: the `ModelDetach` kind.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Detachment {
    pub revision: String,
    pub decision: String,
    pub model: String,
}

/// Appends a minor revision of `decision` on top of its latest to the log of
/// `session`, approved by `approval`, binding `model` (none unbinds), and
/// gives its id.
fn append(
    session: &Session,
    records: &Records,
    decision: &Decision,
    approval: Attribution,
    model: Option<&str>,
    evaluation: Value,
) -> Result<String> {
    let key = decision.key();
    Ok(append_revision(
        &session.root,
        RevisionSpec {
            decision: key.to_owned(),
            parent: records.latest(key).map(|latest| latest.id.clone()),
            meaning_version: Some(decision.meaning_version()),
            check_revision: Some(decision.check_revision()),
            change: Change::Minor,
            evaluation,
            approval,
            model: model.map(str::to_owned),
            generated_at_time: now(),
        },
    )?)
}

/// Evaluates `request.model` as the first layer of the decision and, when the
/// evaluation allows it, appends the revision that attaches it.
pub fn attach(request: &EvalRequest, approval: Attribution) -> Result<Attachment> {
    let model = request
        .model
        .clone()
        .ok_or("--model names the model to attach")?;
    let session = Session::load(request.labelling.config.as_deref())?;
    let decision = session
        .catalog
        .decision(&request.decision)
        .ok_or_else(|| format!("unknown decision `{}`", request.decision))?;
    let records = Records::read(&session.root)?;
    let report = eval(request)?;
    let evaluation = evidence(&report)?;
    let mut reasons = attachable(&report);
    if records.attached().get(decision.key()) == Some(&model) {
        reasons.push(format!("`{model}` is already attached"));
    }
    let revision = if reasons.is_empty() {
        Some(append(
            &session,
            &records,
            decision,
            approval,
            Some(&model),
            evaluation.clone(),
        )?)
    } else {
        None
    };
    Ok(Attachment {
        decision: decision.id().to_owned(),
        model,
        revision,
        reasons,
        evaluation,
        report,
    })
}

/// Appends the revision that unbinds the model of `decision`.
pub fn detach(config: Option<&Path>, decision: &str, approval: Attribution) -> Result<Detachment> {
    let session = Session::load(config)?;
    let found = session
        .catalog
        .decision(decision)
        .ok_or_else(|| format!("unknown decision `{decision}`"))?;
    let records = Records::read(&session.root)?;
    let model = records
        .attached()
        .remove(found.key())
        .ok_or_else(|| format!("no model is attached to `{decision}`"))?;
    let revision = append(
        &session,
        &records,
        found,
        approval,
        None,
        json!({ "detached": model }),
    )?;
    Ok(Detachment {
        revision,
        decision: found.id().to_owned(),
        model,
    })
}
