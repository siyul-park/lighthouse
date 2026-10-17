//! `lighthouse model train`: collects the labelled subjects of a decision the
//! way evaluation does, in each repository the labels are about, and hands
//! them to the host's training (`lighthouse_ml::train`).

use lighthouse_engine::ModelOptions;
use lighthouse_ml::train::{Fit, Rows, Trained};
use lighthouse_plugin::DecisionRef;

use super::{Labelling, Repo, examples, labelling};
use crate::{CheckRequest, Checked, Result, Session, check};

/// What to train.
pub struct TrainRequest {
    pub decision: String,
    /// The model to train (`<provider>/<model>`); else the decision's bound
    /// model.
    pub model: Option<String>,
    /// A model whose answers on unlabelled subjects are labels too, at the
    /// weight of a model: a `fail` where it is sure of one, a `pass` where it
    /// is sure of the other.
    pub from_model: Option<String>,
    pub labelling: Labelling,
}

/// What the repositories gave to a training run: the rows of all of them, and
/// what the first one trains with.
pub(super) struct Labelled {
    pub(super) rows: Rows,
    /// The model trained: `model`, else the one bound to the decision.
    pub(super) reference: Option<String>,
    pub(super) decision: DecisionRef,
    /// The first repository's run, which holds the models, and its store.
    pub(super) checked: Checked,
    pub(super) store: std::path::PathBuf,
}

/// Trains the model on the labels of the decision.
pub fn train(request: &TrainRequest) -> Result<Trained> {
    let labelled = labelled(
        &request.labelling,
        &request.decision,
        request.model.as_deref(),
        request.from_model.as_deref(),
    )?;
    let model = labelled
        .reference
        .as_deref()
        .map(|r| labelled.checked.model(r))
        .transpose()?;
    let fit = Fit {
        root: &labelled.store,
        reference: labelled.reference.as_deref(),
        decision: &labelled.decision,
        model: model.as_deref(),
    };
    lighthouse_ml::train::train(labelled.rows, &fit)
}

/// The labelled subjects of `decision` in each repository the labels are
/// about, for the model `model` and, when given, taught by `from_model`.
pub(super) fn labelled(
    labelling: &Labelling,
    decision: &str,
    model: Option<&str>,
    from_model: Option<&str>,
) -> Result<Labelled> {
    let mut first: Option<Labelled> = None;
    for (at, (repo, session)) in labelling.sessions()?.iter().enumerate() {
        let one = gather(
            labelling,
            repo,
            session,
            decision,
            model,
            from_model,
            at == 0,
        )?;
        match first.as_mut() {
            Some(first) => first.rows.extend(one.rows),
            None => first = Some(one),
        }
    }
    first.ok_or_else(|| "nothing to train on".into())
}

/// The labelled subjects of the repository of `session`. The examples are
/// subjects in the first repository only.
fn gather(
    labelling: &Labelling,
    repo: &Repo,
    session: &Session,
    decision: &str,
    model: Option<&str>,
    from_model: Option<&str>,
    first: bool,
) -> Result<Labelled> {
    let binding = session.config.models(session.catalog.projects())?;
    let checked = check(
        session,
        &CheckRequest {
            rules: vec![decision.to_owned()],
            store: false,
            cache: false,
            models: ModelOptions {
                off: true,
                collect: true,
                ..ModelOptions::default()
            },
            ..CheckRequest::default()
        },
    )?;
    let found = checked
        .catalog
        .decision(decision)
        .ok_or_else(|| format!("unknown decision `{decision}`"))?;
    let reference = model
        .map(str::to_owned)
        .or_else(|| {
            let route = binding.route(decision);
            route.and_then(|models| models.first()).cloned()
        })
        .or_else(|| binding.classification.first().cloned());
    let examples = if first {
        let models = ModelOptions {
            off: true,
            ..ModelOptions::default()
        };
        examples::labels(session, found, decision, &models)?
    } else {
        Vec::new()
    };
    let table = labelling.table_of(repo);
    let labels = labelling::labels(
        session,
        found,
        table.as_ref(),
        reference.as_deref(),
        examples,
    )?;
    let decision_ref = found.reference();
    let teacher = from_model
        .map(|teacher| checked.model(teacher))
        .transpose()?;
    let rows = Rows::of(
        &labels,
        &checked.run.questions,
        teacher.as_deref(),
        &decision_ref,
    )?;
    drop(teacher);
    Ok(Labelled {
        rows,
        reference,
        decision: decision_ref,
        store: session.store.clone(),
        checked,
    })
}
