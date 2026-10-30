//! Fixing: the findings a request selects are fixed by the fixers the
//! catalog names, verified, written and remembered. The engine's orchestrator
//! does the work; this adds what only a session knows: the catalog, the
//! judgments that keep findings out of reach, and the store.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_engine::{
    AppliedFix, BatchTrace, DeclinedFix, Engine, FixPlan, FixRun, FixedBy, ModelCounts,
    ModelOptions, PreviewedFix, ToolCounts, cache, unified_diff,
};
use lighthouse_model::{Fingerprint, Preview, slashed};
use lighthouse_protocol::ToolEvent;
use lighthouse_report::{Aim, Apply, FixBy};
use lighthouse_spec::{Catalog, Decision};
use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    Result, Selector, Session,
    check::cache_limit,
    descriptor::{fix_by, fix_plan},
    findings,
    selector::long_enough,
};

/// Which findings to fix. At least one of `paths`, `fingerprints` and `rules`
/// narrows a run from the whole project.
#[derive(Default)]
pub struct Targets {
    pub paths: Vec<PathBuf>,
    /// Fingerprints or unambiguous prefixes.
    pub fingerprints: Vec<String>,
    pub rules: Vec<String>,
}

/// What to fix, and how.
#[derive(Default)]
pub struct Selection {
    pub targets: Targets,
    /// Do everything but leave the files as they were.
    pub dry_run: bool,
    /// Whether to write `unsafe` fixes: `Some(true)` does (`--unsafe-fixes`),
    /// `Some(false)` only previews them, `None` leaves it to `[fix] unsafe`.
    /// With fingerprints, `Some(true)` applies the previews of those findings,
    /// exactly as they were shown.
    pub unsafe_fixes: Option<bool>,
    /// Use this registered fixer instead of the catalog's.
    pub fixer: Option<String>,
    /// Record the run and its fixes, and leave out what judgments hide.
    pub store: bool,
    /// What models may do. Fixes by a model ask the project's `[models] agent`
    /// unless `off`.
    pub models: ModelOptions,
    /// Keep the answers of models in `.lighthouse/cache` and reuse them.
    pub cache: bool,
}

/// A finished fix run: the `FixReport` kind.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fixed {
    pub dry_run: bool,
    /// The unified diff of every changed file, in path order.
    pub diff: String,
    pub applied: Vec<Applied>,
    pub declined: Vec<DeclinedFix>,
    /// How many times fixing was repeated until nothing was left.
    pub rounds: usize,
    /// `unsafe` fixes, by any provider, that the checks accepted and that were
    /// left for a hand to apply (`[fix] unsafe = false` without
    /// `--unsafe-fixes`, or a failing `[fix] verify` command), each with its
    /// diff and how to apply it. Absent when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previews: Vec<Previewed>,
    /// What became of the findings given to a model, by decision. Absent when
    /// no model was asked.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelCounts>,
    /// The requests that went to a model this run, and what they cost.
    /// Absent when no model was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// What the user should know, in the order it happened.
    pub messages: Vec<String>,
    /// What every fix left as a preview changes, by fingerprint, and in a dry
    /// run every fix that would be applied: the SARIF `fixes` of a report. Each
    /// is relative to the original text, so that it applies alone; the one
    /// made last for a fingerprint stands for all of them.
    #[serde(skip)]
    pub shown: BTreeMap<String, Preview>,
}

/// An `unsafe` fix that was left for a hand to apply.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Previewed {
    pub fingerprint: String,
    pub decision: String,
    pub fixer: String,
    pub safety: String,
    pub description: String,
    pub files: Vec<String>,
    /// The unified diff of the fix.
    pub diff: String,
    /// Applies exactly this fix, as a command line and as a tool call.
    #[serde(flatten)]
    pub apply: Apply,
}

/// Requests to models and what they cost.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Usage {
    /// Requests that went to a model (a run of an agent is one).
    #[serde(flatten)]
    pub usage: lighthouse_model::Usage,
    /// Present when the model is an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Runs>,
}

/// What the runs of an agent took.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Runs {
    /// Runs of the agent; answers kept from earlier runs are not counted.
    pub runs: usize,
    pub seconds: f64,
    /// What each run did, from the events its harness reported; absent when
    /// the provider declares none. A run that was kept from an earlier one has
    /// no entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub batches: Vec<Batch>,
}

/// What an agent did for the findings of one file. A measurement: it never
/// decides a fix.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Batch {
    /// The file the findings are in.
    pub file: String,
    /// Files it read that the brief did not include, in the order it opened
    /// them, each once; every file outside the project is one entry,
    /// `(outside the project)`.
    pub reads: Vec<String>,
    /// What it searched, and the listings it made, in order.
    pub searches: Vec<ToolEvent>,
    /// How many calls of each tool it made.
    #[serde(rename = "calls")]
    pub counts: ToolCounts,
    /// This is not all the agent did: `note` says what is missing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
    /// What is missing from this, when `incomplete`: output that was cut,
    /// events past the count kept, events that were not understood.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl From<&BatchTrace> for Batch {
    fn from(trace: &BatchTrace) -> Self {
        Self {
            file: slashed(&trace.file),
            reads: trace.reads.clone(),
            searches: trace.searches.clone(),
            counts: trace.counts,
            incomplete: trace.incomplete.is_some(),
            note: trace.incomplete.clone(),
        }
    }
}

/// A fix that was applied.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Applied {
    pub fingerprint: String,
    pub decision: String,
    pub fixer: String,
    pub safety: String,
    pub description: String,
    pub files: Vec<String>,
    /// A safe fix of what the change of other fixes introduced, in a file they
    /// all edited: the part of the change that is not theirs. In a file one fix
    /// edited it is part of that fix.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub follow_up: bool,
}

impl Fixed {
    /// The property bag a SARIF log of the run carries: what the agent did,
    /// under `agent`. Empty when no agent ran.
    pub fn properties(&self) -> serde_json::Map<String, serde_json::Value> {
        let agent = self.usage.as_ref().and_then(|usage| usage.agent.as_ref());
        agent
            .and_then(|agent| serde_json::to_value(agent).ok())
            .map(|agent| serde_json::Map::from_iter([("agent".to_owned(), agent)]))
            .unwrap_or_default()
    }

    /// What a person is told about the run, a line each: the messages, every
    /// fix applied (or that would be) and every finding left alone, and the
    /// count.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = self.messages.clone();
        let verb = if self.dry_run { "would fix" } else { "fixed" };
        for fix in &self.applied {
            let follow = if fix.follow_up { " (follow-up)" } else { "" };
            lines.push(format!(
                "{verb} {} [{}] {} ({}): {}{follow}",
                fix.decision,
                fix.safety,
                fix.files.join(", "),
                fix.fixer,
                fix.description
            ));
        }
        for fix in &self.previews {
            lines.push(format!(
                "preview {} [{}] {}: {}",
                fix.decision,
                fix.safety,
                fix.files.join(", "),
                fix.description
            ));
            lines.extend(fix.diff.lines().map(|line| format!("  {line}")));
            lines.push(format!("  apply: {}", fix.apply.apply));
        }
        for fix in &self.declined {
            lines.push(format!(
                "not fixed {}:{} {}: {}",
                slashed(&fix.file),
                fix.line,
                fix.rule_id,
                fix.reason
            ));
        }
        lines.extend(
            self.models
                .iter()
                .map(|(decision, counts)| model_line(counts, decision)),
        );
        if let Some(usage) = &self.usage {
            let runs = usage.agent.as_ref().map_or(String::new(), |agent| {
                format!(", {} run(s) in {:.1}s", agent.runs, agent.seconds)
            });
            let reads: usize = usage
                .agent
                .iter()
                .flat_map(|agent| &agent.batches)
                .map(|batch| batch.reads.len())
                .sum();
            let runs = if reads == 0 {
                runs
            } else {
                format!("{runs}, {reads} file(s) read beyond the brief")
            };
            let cut = usage
                .agent
                .iter()
                .flat_map(|agent| &agent.batches)
                .filter(|batch| batch.incomplete)
                .count();
            let runs = if cut == 0 {
                runs
            } else {
                format!("{runs}, what the agent did is incomplete for {cut} batch(es)")
            };
            lines.push(format!(
                "models: {} request(s), cost {}{runs}",
                usage.usage.calls, usage.usage.cost
            ));
        }
        let waiting = if self.previews.is_empty() {
            String::new()
        } else {
            format!(", {} awaiting a hand", self.previews.len())
        };
        let follow_ups = self.applied.iter().filter(|fix| fix.follow_up).count();
        let tidied = if follow_ups == 0 {
            String::new()
        } else {
            format!(" and {follow_ups} follow-up(s)")
        };
        lines.push(format!(
            "{} fix(es){tidied} {}, {} left alone{waiting}",
            self.applied.len() - follow_ups,
            if self.dry_run { "proposed" } else { "applied" },
            self.declined.len()
        ));
        lines
    }
}

impl Previewed {
    /// The fix `fix` as reported, its application naming the findings it was
    /// made for by the first characters of their fingerprints that tell them
    /// apart, `widths` of each.
    fn new(fix: &PreviewedFix, plan: &FixPlan, widths: &BTreeMap<&str, usize>) -> Self {
        let preview = &fix.preview;
        let fingerprint = preview.fingerprint.as_str();
        let batch: Vec<&str> = fix
            .batch
            .iter()
            .map(|member| {
                let member = member.as_str();
                member
                    .get(..widths.get(member).copied().unwrap_or(member.len()))
                    .unwrap_or(member)
            })
            .collect();
        let diff = preview
            .files
            .iter()
            .map(|file| format!("--- a/{0}\n+++ b/{0}\n{1}\n", file.path, file.hunks))
            .collect();
        Self {
            fingerprint: fingerprint.to_owned(),
            decision: fix.rule_id.clone(),
            fixer: by(plan, &fix.rule_id, &fix.fixer),
            safety: preview.safety.to_string(),
            description: preview.description.clone(),
            files: preview.files.iter().map(|f| f.path.clone()).collect(),
            diff,
            apply: Apply::of(Aim::Findings(&batch), true),
        }
    }
}

impl Applied {
    /// The fix `fix` as reported; `plan` says who makes the fixes.
    fn new(fix: &AppliedFix, plan: &FixPlan) -> Self {
        Self {
            fingerprint: fix.fingerprint.as_str().to_owned(),
            decision: fix.rule_id.clone(),
            fixer: by(plan, &fix.rule_id, &fix.fixer),
            safety: fix.safety.to_string(),
            description: fix.description.clone(),
            files: fix.files.iter().map(|f| slashed(f)).collect(),
            follow_up: fix.follow_up,
        }
    }
}

/// Fixes what `request` selects. With `dry_run` nothing is left changed and
/// nothing is recorded; otherwise every applied fix is recorded and the
/// findings it removed are resolved in the store.
pub fn run(session: &Session, request: &Selection) -> Result<Fixed> {
    for prefix in &request.targets.fingerprints {
        long_enough(prefix)?;
    }
    let catalog = &session.catalog;
    let root = &session.root;
    let (engine, notices) = session.engine()?;
    let plan = fix_plan(session, &|id| engine.active(id))?;
    let engine = engine.with_models(request.models.clone());
    // The answers of models and the previews a run leaves are kept in the
    // cache.
    let engine = if request.cache {
        engine.with_cache(cache::dir(root), cache_limit())
    } else {
        engine
    };
    let mut messages = notices;
    let skip = suppressed(&engine, root, catalog, request.store)?;
    let run = FixRun {
        paths: request.targets.paths.clone(),
        rules: Selector::new(&request.targets.rules)?.expand(
            catalog
                .decisions()
                .map(Decision::id)
                .filter(|id| engine.active(id)),
        )?,
        fingerprints: request.targets.fingerprints.clone(),
        skip,
        dry_run: request.dry_run,
        unsafe_fixes: request.unsafe_fixes,
        fixer: request.fixer.clone(),
    };
    let mut report = engine.fix(&plan, &run)?;
    messages.extend(report.notes.iter().cloned());
    if request.store && !request.dry_run {
        let remembered = findings::remember(root, catalog, &mut report.finished);
        messages.extend(remembered.messages);
    }
    let diff = report
        .changes
        .iter()
        .map(|c| unified_diff(&c.path, &c.before, &c.after))
        .collect();
    let widths = shortest(&report);
    let shown = report
        .applied
        .iter()
        .filter_map(|fix| fix.preview.clone())
        .chain(report.previews.iter().map(|fix| fix.preview.clone()))
        .map(|preview| (preview.fingerprint.as_str().to_owned(), preview))
        .collect();
    Ok(Fixed {
        dry_run: request.dry_run,
        diff,
        applied: report
            .applied
            .iter()
            .map(|fix| Applied::new(fix, &plan))
            .collect(),
        declined: report.declined.clone(),
        rounds: report.rounds,
        previews: report
            .previews
            .iter()
            .map(|fix| Previewed::new(fix, &plan, &widths))
            .collect(),
        models: report.models.clone(),
        usage: (!report.models.is_empty()).then_some(Usage {
            usage: report.usage,
            agent: report.model_edits.as_ref().map(|agent| Runs {
                runs: agent.runs,
                seconds: agent.seconds,
                batches: agent.batches.iter().map(Batch::from).collect(),
            }),
        }),
        messages,
        shown,
    })
}

/// Who made a fix, in the words of the descriptor: `model`, else what the
/// decision's fix is (`cel`, `command`, `rpc`), and `cel` for a fixer the run
/// named itself.
fn by(plan: &FixPlan, rule: &str, fixer: &FixedBy) -> String {
    let word = match plan.get(rule) {
        Some(binding) if binding.fixer == *fixer => fix_by(&binding.needs),
        _ if *fixer == FixedBy::Model => FixBy::Model,
        _ => FixBy::Cel,
    };
    word.as_str().to_owned()
}

/// How many characters of each fingerprint of the run and its previews tell it
/// apart from the others, by fingerprint.
fn shortest(report: &lighthouse_engine::FixReport) -> BTreeMap<&str, usize> {
    let all: BTreeSet<&str> = report
        .finished
        .diagnostics
        .iter()
        .map(|d| d.fingerprint.as_str())
        .chain(
            report
                .previews
                .iter()
                .flat_map(|p| p.batch.iter().map(Fingerprint::as_str)),
        )
        .collect();
    let all: Vec<&str> = all.into_iter().collect();
    let widths = Fingerprint::prefix_widths(&all);
    all.into_iter().zip(widths).collect()
}

/// Reads the project as it is before any fix, recording it when the run uses
/// the store, and returns the
/// findings that judgments keep out of reports: a fixer leaves those alone.
fn suppressed(
    engine: &Engine,
    root: &std::path::Path,
    catalog: &Catalog,
    store: bool,
) -> Result<BTreeSet<Fingerprint>> {
    let mut outcome = engine.check(&[], &[])?;
    let all: BTreeSet<Fingerprint> = outcome
        .diagnostics
        .iter()
        .map(|d| d.fingerprint.clone())
        .collect();
    if store {
        findings::remember(root, catalog, &mut outcome);
    } else {
        findings::apply_judgments(root, catalog, &mut outcome)?;
    }
    let kept: BTreeSet<Fingerprint> = outcome
        .diagnostics
        .iter()
        .map(|d| d.fingerprint.clone())
        .collect();
    Ok(all.difference(&kept).cloned().collect())
}

/// A person's line for the counts of `decision`.
fn model_line(counts: &ModelCounts, decision: &str) -> String {
    {
        let declined: Vec<String> = counts
            .declined
            .iter()
            .map(|(class, n)| format!("{class} {n}"))
            .collect();
        let declined = if declined.is_empty() {
            "0".to_owned()
        } else {
            format!(
                "{} ({})",
                counts.declined.values().sum::<usize>(),
                declined.join(", ")
            )
        };
        format!(
            "model fixes {decision}: asked {}, answered {}, verified {}, applied {}, previewed {}, declined {declined}",
            counts.asked, counts.answered, counts.verified, counts.applied, counts.previewed
        )
    }
}
