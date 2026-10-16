//! `lighthouse model rebuild`: makes the artifacts of the attached models that
//! a machine does not have, from the labels they learned from. A model is a
//! file named by its digest, kept out of git; a fresh clone has the log that
//! attaches it and not the file.

use std::path::{Path, PathBuf};

use lighthouse_ml::{
    evaluate::Repo,
    rebuild::{
        Artifact, Learned, Refit, Status, artifact, has_dataset, learned, refit, unrebuildable,
    },
    train::{TrainRequest, write_dataset},
};
use lighthouse_spec::Decision;
use lighthouse_store::Records;
use serde::Serialize;

use crate::{
    Result, Selector, Session,
    attach::key_of,
    attached_models,
    train::{Collected, collect},
};

/// What to rebuild.
pub struct RebuildRequest {
    /// Decision ids or globs; every decision with an attached model when empty.
    pub decisions: Vec<String>,
    pub baseline: Option<PathBuf>,
    /// The repositories the labels are about, as in `model train`.
    pub repos: Vec<Repo>,
    pub config: Option<PathBuf>,
}

/// How the model attached to a decision came out.
#[derive(Debug, Serialize)]
pub struct Rebuilt {
    pub decision: String,
    pub model: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Makes the missing artifact of every selected attached model again.
pub fn rebuild(request: &RebuildRequest) -> Result<Vec<Rebuilt>> {
    let session = Session::load(request.config.as_deref())?;
    let catalog = session.catalog()?;
    let selector = Selector::new(&request.decisions)?;
    let records = Records::read(&session.root)?;
    let mut rows = Vec::new();
    for (id, model) in attached_models(&session.root, &catalog) {
        let Some(decision) = catalog.decision(&id).filter(|_| selector.matches(&id)) else {
            continue;
        };
        let (status, detail) = restore(request, &session.root, &records, decision, &model)?;
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
    root: &Path,
    records: &Records,
    decision: &Decision,
    model: &str,
) -> Result<(Status, Option<String>)> {
    let Some(learned) = learned(model) else {
        let note = "a service, not a file";
        return Ok((Status::NotLocal, Some(note.to_owned())));
    };
    match artifact(root, learned.hex)? {
        Artifact::Present => return Ok((Status::Present, None)),
        Artifact::Differs(actual) => {
            let note = format!("the file holds sha256:{actual}, not sha256:{}", learned.hex);
            return Ok((Status::Differs, Some(note)));
        }
        Artifact::Missing => {}
    }
    let Some(attached) = trained_on(records, decision, model) else {
        let note = "the revision that attached it names no training file";
        return Ok((Status::NotRebuildable, Some(note.to_owned())));
    };
    let collected = collect(&TrainRequest {
        decision: decision.id().to_owned(),
        model: Some(learned.trainer.to_owned()),
        from_model: None,
        baseline: request.baseline.clone(),
        repos: request.repos.clone(),
        config: request.config.clone(),
    })?;
    train_again(root, decision, &learned, &attached, collected)
}

/// Trains the model on the dataset it was attached with: the one in the
/// store, else the one today's labels make when they make the same.
fn train_again(
    root: &Path,
    decision: &Decision,
    learned: &Learned,
    attached: &str,
    collected: Collected,
) -> Result<(Status, Option<String>)> {
    let first = &collected.first;
    let trainer = first.checked.model(learned.trainer)?;
    if let Some(why) = unrebuildable(trainer.as_ref()) {
        return Ok((Status::NotRebuildable, Some(why)));
    }
    if !has_dataset(root, attached) {
        let made = write_dataset(collected.rows, root, &first.decision_ref)?;
        if made != attached {
            let note = format!(
                "labels changed since attach (dataset {made}, attached from {attached}); train and attach again"
            );
            return Ok((Status::LabelsChanged, Some(note)));
        }
    }
    refit(
        trainer.as_ref(),
        &Refit {
            root,
            decision: decision.id(),
            model: learned,
            dataset: attached,
        },
    )
}

/// The dataset the revision that attached `model` to `decision` names.
fn trained_on(records: &Records, decision: &Decision, model: &str) -> Option<String> {
    let key = key_of(decision);
    let attaching = records
        .revisions
        .iter()
        .rev()
        .find(|r| r.spec.decision == key && r.spec.model.as_deref() == Some(model))?;
    let file = attaching.spec.evaluation.get("trainingFile")?;
    file.as_str().map(str::to_owned)
}
