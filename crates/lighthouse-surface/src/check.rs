//! `check`: analyze the project, optionally fix what can be fixed, and report.

use std::time::Instant;

use lighthouse_report::{Briefing, Format, agent_report, render_with};
use lighthouse_session::{CheckRequest, Checked, FailOn, ModelFlags, Session};
use serde_json::{Value, json};

use crate::{
    Result,
    args::{CheckArgs, Detail, FixArgs, FixOptions},
    fix,
    reply::{CheckFormat, Context, Reply},
    timings,
};

/// Most findings of a SARIF report whose fix is previewed.
const SARIF_FIXES: usize = 200;
/// Findings of a JSON report when `--limit` is not given.
const JSON_LIMIT: usize = 25;

/// What `check` is asked to print besides the findings.
#[derive(Debug, Default, Clone)]
pub struct Printing {
    pub format: CheckFormat,
    /// Print where the time went, to stderr.
    pub timings: bool,
    /// Fix first; `None` only checks.
    pub fixing: Option<FixOptions>,
}

/// Checks the project, fixing first when asked, and reports. The exit code
/// says whether the findings fail the run.
pub fn run(ctx: &Context, args: &CheckArgs, printing: &Printing) -> Result<Reply> {
    let started = Instant::now();
    if args.changed && args.diff.is_some() {
        return Err("`changed` and `diff` are alternatives".into());
    }
    let mut notes = Vec::new();
    let mut fixed = None;
    if let Some(options) = &printing.fixing {
        let selection = FixArgs {
            paths: args.paths.clone(),
            decision: args.decision.clone(),
            fingerprints: Vec::new(),
            options: options.clone(),
        };
        let done = fix::apply(ctx, &selection, !args.no_store)?;
        notes.extend(fix::notes(&done));
        if options.dry_run {
            let reply = report_dry_run(&done, printing)?;
            return Ok(reply.with_notes(notes));
        }
        fixed = Some(done);
    }
    let checked = analyze(ctx, args)?;
    notes.extend(checked.messages.iter().map(|m| format!("lighthouse: {m}")));
    let reporting = Instant::now();
    let mut reply = match (&fixed, printing.format) {
        (Some(done), CheckFormat::Json) => Reply::json(serde_json::to_value(fix::report(done))?)?,
        (_, CheckFormat::Json) => Reply::json(json_report(&checked, args))?,
        (_, CheckFormat::Sarif) => Reply::text(sarif(&checked, args)),
        (_, CheckFormat::Text) => Reply::text(text(&checked, args)),
    };
    if printing.timings {
        notes.extend(timings::lines(
            &checked.outcome.timings,
            reporting.elapsed(),
            started.elapsed(),
        ));
    }
    let code = checked.outcome.exit_code(
        FailOn {
            strict: args.strict,
            max_warnings: args.max_warnings,
        },
        args.allow_incomplete,
    );
    reply.notes = notes;
    reply.code = code;
    Ok(reply)
}

/// The reply of a dry run: the diff is the data.
fn report_dry_run(done: &lighthouse_session::Fixed, printing: &Printing) -> Result<Reply> {
    match printing.format {
        CheckFormat::Json => Reply::json(serde_json::to_value(fix::report(done))?),
        _ => Ok(Reply::text(done.diff.clone())),
    }
}

/// Analyzes the project and keeps what the arguments report.
pub fn analyze(ctx: &Context, args: &CheckArgs) -> Result<Checked> {
    let session = Session::load(ctx.config.as_deref())?;
    let request = CheckRequest {
        paths: ctx.paths(&session.root, &args.paths)?,
        changed: args.changed,
        diff: args.diff.clone(),
        rules: args.decision.clone(),
        store: !args.no_store,
        cache: !args.no_cache,
        models: ModelFlags {
            off: args.no_models,
            all: args.all_models,
            record: args.record_model_labels,
            collect: false,
        },
    };
    lighthouse_session::check(session, &request)
}

/// The findings to print: what the check found, then the subjects nothing
/// decided, which are reported beside them as `review` results and never
/// change the exit code.
fn shown(checked: &Checked) -> Vec<lighthouse_model::Diagnostic> {
    let outcome = &checked.outcome;
    outcome
        .diagnostics
        .iter()
        .chain(&outcome.reviews)
        .cloned()
        .collect()
}

fn text(checked: &Checked, args: &CheckArgs) -> String {
    let outcome = &checked.outcome;
    render_with(
        Format::Text,
        &shown(checked),
        &outcome.incomplete,
        &checked.briefing(args.limit),
    )
}

fn sarif(checked: &Checked, args: &CheckArgs) -> String {
    let outcome = &checked.outcome;
    let all: Vec<_> = outcome.diagnostics.iter().take(SARIF_FIXES).collect();
    let fixes = checked.fixes(&all);
    let sources = checked.sources();
    render_with(
        Format::Sarif,
        &shown(checked),
        &outcome.incomplete,
        &Briefing {
            fixes: Some(&fixes),
            sources: Some(&sources),
            ..checked.briefing(args.limit)
        },
    )
}

/// The `CheckReport` kind: the findings grouped by decision for a program,
/// with the gaps and the counts, and what the run has to say.
pub fn json_report(checked: &Checked, args: &CheckArgs) -> Value {
    let outcome = &checked.outcome;
    let limit = Some(args.limit.unwrap_or(JSON_LIMIT));
    let detail = args.detail.into();
    let fixes = checked.shown_fixes(limit, detail);
    let report = agent_report(
        &shown(checked),
        &outcome.incomplete,
        &Briefing {
            detail,
            fixes: Some(&fixes),
            sources: None,
            tool_form: true,
            ..checked.briefing(limit)
        },
    );
    let mut body = report.fields;
    match args.detail {
        Detail::Compact => {
            if !checked.messages.is_empty() {
                body.insert("messages".to_owned(), json!(checked.messages));
            }
        }
        Detail::Full => {
            let summary = checked.summary();
            body.insert("status".to_owned(), json!(summary.status));
            body.insert("summary".to_owned(), json!(summary));
            body.insert("messages".to_owned(), json!(checked.messages));
            body.entry("reasons").or_insert(Value::Null);
        }
    }
    Value::Object(body)
}
