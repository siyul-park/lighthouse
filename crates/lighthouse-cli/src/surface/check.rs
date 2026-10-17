//! `check`: analyze the project, optionally fix what can be fixed, and report.

use std::time::Instant;

use lighthouse_report::Format;
use lighthouse_session::{CheckRequest, Checked, FailOn, ModelOptions, Session, fix::Fixed};

use crate::surface::{
    Result,
    args::{CheckArgs, FixArgs, FixOptions},
    fix,
    reply::{CheckFormat, Context, Reply},
};

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
        (_, CheckFormat::Json) => Reply::json(checked.report(args.detail.into(), args.limit))?,
        (_, CheckFormat::Sarif) => Reply::text(checked.render(Format::Sarif, args.limit)),
        (_, CheckFormat::Text) => Reply::text(checked.render(Format::Text, args.limit)),
    };
    if printing.timings {
        notes.extend(
            checked
                .run
                .timings
                .lines(reporting.elapsed(), started.elapsed()),
        );
    }
    let code = checked.run.exit_code(
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

/// Analyzes the project and keeps what the arguments report.
fn analyze(ctx: &Context, args: &CheckArgs) -> Result<Checked> {
    let session = Session::load(ctx.config.as_deref())?;
    let request = CheckRequest {
        paths: ctx.paths(&session.root, &args.paths)?,
        changed: args.changed,
        diff: args.diff.clone(),
        rules: args.decision.clone(),
        store: !args.no_store,
        cache: !args.no_cache,
        models: ModelOptions {
            off: args.no_models,
            all: args.all_models,
            ..ModelOptions::default()
        },
        record_model_labels: args.record_model_labels,
        detached: false,
    };
    lighthouse_session::check(&session, &request)
}

/// The reply of a dry run: the diff is the data.
fn report_dry_run(done: &Fixed, printing: &Printing) -> Result<Reply> {
    match printing.format {
        CheckFormat::Json => Reply::json(serde_json::to_value(fix::report(done))?),
        _ => Ok(Reply::text(done.diff.clone())),
    }
}
