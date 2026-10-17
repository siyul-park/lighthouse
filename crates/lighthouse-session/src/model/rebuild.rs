//! `lighthouse model rebuild`: makes the artifacts of the attached models that
//! a machine does not have, from the labels they learned from. A model is a
//! file named by its digest, kept out of git; a fresh clone has the log that
//! attaches it and not the file.

use std::path::Path;

use lighthouse_ml::rebuild::{Learned, Status};
use lighthouse_spec::Decision;
use lighthouse_store::Records;
use schemars::JsonSchema;
use serde::Serialize;

use super::{Labelling, attached, train::labelled};
use crate::{Result, Selector, Session};

/// What to rebuild.
pub struct RebuildRequest {
    /// Decision ids or globs; every decision with an attached model when empty.
    pub decisions: Vec<String>,
    pub labelling: Labelling,
}

/// How the model attached to a decision came out: a row of the `ModelRebuild`
/// kind.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Rebuilt {
    pub decision: String,
    pub model: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Makes the missing artifact of every selected attached model again.
pub fn rebuild(request: &RebuildRequest) -> Result<Vec<Rebuilt>> {
    let session = Session::load(request.labelling.config.as_deref())?;
    let selector = Selector::new(&request.decisions)?;
    let records = Records::read(&session.root)?;
    let store = request.labelling.sessions()?.remove(0).1.store;
    let session = session.stored_in(&store);
    let mut rows = Vec::new();
    for (id, model) in attached(&session.root, &session.catalog) {
        let Some(decision) = session
            .catalog
            .decision(&id)
            .filter(|_| selector.matches(&id))
        else {
            continue;
        };
        let (status, detail) = restore(request, &session, &records, decision, &model)?;
        rows.push(Rebuilt {
            decision: id,
            model,
            status,
            detail,
        });
    }
    Ok(rows)
}

fn restore(
    request: &RebuildRequest,
    session: &Session,
    records: &Records,
    decision: &Decision,
    model: &str,
) -> Result<(Status, Option<String>)> {
    let Some(learned) = Learned::parse(model) else {
        let note = "a service, not a file";
        return Ok((Status::NotLocal, Some(note.to_owned())));
    };
    let store = &session.store;
    if let Some(found) = learned.check(store)? {
        return Ok(found);
    }
    let Some(attached) = attached_dataset(records, decision, model) else {
        let note = "the revision that attached it names no training file";
        return Ok((Status::NotRebuildable, Some(note.to_owned())));
    };
    let (engine, _) = session.engine()?;
    let trainer = engine.model(learned.trainer)?;
    learned.rebuild(trainer.as_ref(), store, decision.id(), &attached, || {
        today(request, decision, &learned, store)
    })
}

/// The digest of the dataset today's labels make for the model's trainer.
fn today(
    request: &RebuildRequest,
    decision: &Decision,
    learned: &Learned,
    store: &Path,
) -> Result<String> {
    let labelled = labelled(
        &request.labelling,
        decision.id(),
        Some(learned.trainer),
        None,
    )?;
    labelled.rows.write(store, &labelled.decision)
}

/// The dataset the revision that attached `model` to `decision` names.
fn attached_dataset(records: &Records, decision: &Decision, model: &str) -> Option<String> {
    let key = decision.key();
    let attaching = records
        .revisions
        .iter()
        .rev()
        .find(|r| r.spec.decision == key && r.spec.model.as_deref() == Some(model))?;
    let file = attaching.spec.evaluation.get("trainingFile")?;
    file.as_str().map(str::to_owned)
}
