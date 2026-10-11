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
        /// text or json.
        #[arg(long, default_value = "text")]
        format: Output,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

/// The arguments of `eval`.
#[derive(clap::Args)]
pub struct EvalArgs {
    /// The decision, such as design/feature-envy.
    decision: String,
    /// A repository the labels are about, `name=dir`, where `name` is the
    /// baseline's `repo` column. Repeatable; default: this project.
    #[arg(long = "repo", value_name = "NAME=DIR")]
    repos: Vec<String>,
    /// A table of reviewed findings. Default:
    /// baseline/precision/<decision>.tsv when it exists.
    #[arg(long)]
    baseline: Option<PathBuf>,
    /// A decision file whose check is evaluated beside the current one.
    #[arg(long = "check", value_name = "FILE")]
    candidate: Option<PathBuf>,
    /// Write this many selected subjects the check did not flag to a table
    /// for a person to label (`TP` or `FP`), to measure recall.
    #[arg(long, value_name = "N")]
    audit: Option<usize>,
    #[arg(long)]
    config: Option<PathBuf>,
}

/// How a report is printed.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Output {
    Text,
    Json,
}

/// Evaluates the check of a decision and prints the numbers.
pub fn evaluate(args: EvalArgs, format: Output) -> Result<u8> {
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
        decision: args.decision.clone(),
        repos,
        baseline: args.baseline.or_else(|| default_baseline(&args.decision)),
        candidate: args.candidate,
        config: args.config,
        no_models: false,
        audit: args.audit,
    })?;
    if matches!(format, Output::Json) {
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
        format,
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
    if matches!(format, Output::Json) {
        println!("{}", serde_json::to_string_pretty(&trained)?);
        return Ok(0);
    }
    println!(
        "{} subject(s) for {} ({} label(s) found no subject)",
        trained.subjects, trained.decision, trained.unresolved
    );
    for (source, n) in &trained.by_source {
        println!("  {source}: {n}");
    }
    println!("dataset: {}", trained.dataset.display());
    match &trained.result {
        Some(result) => {
            println!("trained {}", trained.model);
            for (name, value) in &result.metrics {
                println!("  {name}: {value}");
            }
            println!("artifact: {}", result.artifact);
        }
        None => println!("no model that learns is bound: nothing was trained"),
    }
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
    if let Some(meets) = report.meets_policy {
        println!(
            "verdict: {} the policy (precision {}, tolerating {:?}) by the lower bound",
            if meets { "meets" } else { "does not meet" },
            report.policy.precision,
            report.policy.tolerate
        );
    }
    if let Some(path) = &report.audit {
        println!(
            "audit: label the rows of {} (verdict TP or FP)",
            path.display()
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
        "  decided by a model: {}, left to review: {}, questions asked: {} (cost {})",
        side.model_decided, side.reviews, side.calls, side.cost
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

/// `baseline/precision/<pack>/<decision>.tsv` when it exists.
fn default_baseline(decision: &str) -> Option<PathBuf> {
    let path = PathBuf::from(format!("baseline/precision/{decision}.tsv"));
    path.is_file().then_some(path)
}
