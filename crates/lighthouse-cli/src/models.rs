//! `lighthouse eval` and `lighthouse learn train`: measuring a decision's
//! check against its labels, and training a model on them.

use std::path::PathBuf;

use clap::Subcommand;
use lighthouse_session::{
    evaluate::{EvalReport, EvalRequest, Repo, Side, eval},
    learn::{TrainRequest, train},
    metrics::Rate,
};

use crate::Result;

/// What `learn` does.
#[derive(Subcommand)]
pub enum LearnCommand {
    /// Fit a model on the labels of a decision: its examples, the baseline
    /// rows, the judgments recorded under its current meaning version, and
    /// optionally the answers of another model. Writes the artifact under
    /// `.lighthouse/models/`; it binds nothing.
    Train {
        /// The decision, such as design/feature-envy.
        decision: String,
        /// The model to train, `<provider>/<model>`. Default: the one bound
        /// to the decision.
        #[arg(long)]
        model: Option<String>,
        /// Also label the subjects nobody labelled with this model's answers
        /// (weight 0.5), `<provider>/<model>`.
        #[arg(long)]
        from_model: Option<String>,
        /// A table of reviewed findings (`fingerprint repo location verdict`).
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Use only the baseline rows of this repository.
        #[arg(long)]
        repo: Option<String>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

/// The arguments of `eval`.
pub struct Eval {
    pub decision: String,
    pub repos: Vec<String>,
    pub baseline: Option<PathBuf>,
    pub candidate: Option<PathBuf>,
    pub no_models: bool,
    pub json: bool,
    pub config: Option<PathBuf>,
}

/// Evaluates the check of a decision and prints the numbers.
pub fn evaluate(args: Eval) -> Result<u8> {
    let repos = args
        .repos
        .iter()
        .map(|spec| {
            let (name, dir) = spec
                .split_once('=')
                .ok_or_else(|| format!("--repo `{spec}` is not name=dir"))?;
            Ok(Repo {
                name: Some(name.to_owned()),
                dir: PathBuf::from(dir),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let report = eval(&EvalRequest {
        decision: args.decision,
        repos,
        baseline: args.baseline,
        candidate: args.candidate,
        config: args.config,
        no_models: args.no_models,
    })?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(0)
}

/// Trains and prints the metrics the trainer measured.
pub fn learn(command: LearnCommand) -> Result<u8> {
    let LearnCommand::Train {
        decision,
        model,
        from_model,
        baseline,
        repo,
        json,
        config,
    } = command;
    let trained = train(&TrainRequest {
        decision,
        model,
        from_model,
        baseline,
        repo,
        config,
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&trained)?);
        return Ok(0);
    }
    println!(
        "trained {} on {} subject(s) ({} label(s) found no subject)",
        trained.model, trained.subjects, trained.unresolved
    );
    for (source, n) in &trained.by_source {
        println!("  {source}: {n}");
    }
    for (name, value) in &trained.result.metrics {
        println!("  {name}: {value}");
    }
    println!("version: {}", trained.result.version);
    Ok(0)
}

fn print_report(report: &EvalReport) {
    println!(
        "{}: {} labelled subject(s); target precision {} (tolerates {:?})",
        report.decision, report.labelled, report.policy.precision, report.policy.tolerate
    );
    print_side("current", &report.current);
    if let Some(candidate) = &report.candidate {
        print_side("candidate", candidate);
    }
    if let Some(rate) = &report.agreement {
        println!(
            "agreement of the candidate with the current check: {}",
            show(Some(*rate))
        );
    }
    if let Some(meets) = report.meets_precision {
        println!(
            "precision {} the target by the lower bound",
            if meets { "reaches" } else { "does not reach" }
        );
    }
    println!("note: {}", report.caveat);
}

fn print_side(label: &str, side: &Side) {
    let m = &side.metrics;
    println!("{label} ({}):", side.check);
    for (name, rate) in [
        ("precision", m.precision),
        ("FPR", m.fpr),
        ("recall", m.recall),
        ("FNR", m.fnr),
        ("selector precision", m.selector_precision),
        ("coverage", m.coverage),
        ("abstention", m.abstention),
    ] {
        println!("  {name}: {}", show(rate));
    }
    println!(
        "  decided by a model: {}, left to review: {}",
        side.model_decided, side.reviews
    );
}

fn show(rate: Option<Rate>) -> String {
    rate.map_or_else(
        || "-".to_owned(),
        |r| {
            format!(
                "{:.3} [{:.3}, {:.3}] ({}/{})",
                r.value, r.low, r.high, r.hits, r.of
            )
        },
    )
}
