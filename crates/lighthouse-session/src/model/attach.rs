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

impl Revision<'_> {
    /// Appends the revision to the log under `root` and gives its id.
    fn append(self, root: &Path) -> Result<String> {
        let key = self.decision.key();
        Ok(append_revision(
            root,
            RevisionSpec {
                decision: key.to_owned(),
                parent: self.records.latest(key).map(|latest| latest.id.clone()),
                meaning_version: Some(self.decision.meaning_version()),
                check_revision: Some(self.decision.check_revision()),
                change: Change::Minor,
                evaluation: self.evaluation,
                approval: self.approval,
                model: self.model.map(str::to_owned),
                generated_at_time: now(),
            },
        )?)
    }
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
        let revision = Revision {
            records: &records,
            decision,
            approval,
            model: Some(&model),
            evaluation: evaluation.clone(),
        };
        Some(revision.append(&session.root)?)
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
    let revision = Revision {
        records: &records,
        decision: found,
        approval,
        model: None,
        evaluation: json!({ "detached": model }),
    }
    .append(&session.root)?;
    Ok(Detachment {
        revision,
        decision: found.id().to_owned(),
        model,
    })
}
