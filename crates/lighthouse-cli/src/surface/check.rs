//! `check`, and `fix`: analyze the project, apply the fixes the catalog names,
//! verified, and report.

use std::{collections::BTreeMap, time::Instant};

use lighthouse_report::Format;
use lighthouse_session::{
    CheckRequest, Checked, FailOn, ModelOptions, Session,
    fix::{self, Fixed, Selection, Targets},
};

use crate::surface::{
    Result,
    args::{CheckArgs, FixArgs},
    commands::CheckCommand,
    reply::{self, CheckFormat, Context, Reply},
};

/// Checks the project, fixing first when asked, and reports. The exit code
/// says whether the findings fail the run. With `--fix` the report is that of
/// the project after the fix, and the fix run is told in its `messages`.
pub fn check(ctx: &Context, command: &CheckCommand) -> Result<Reply> {
    let started = Instant::now();
    let args = &command.args;
    if args.changed && args.diff.is_some() {
        return Err("`changed` and `diff` are alternatives".into());
    }
    let mut messages = Vec::new();
    let mut shown = BTreeMap::new();
    if command.fix {
        let selection = FixArgs {
            paths: args.paths.clone(),
            decision: args.decision.clone(),
            fingerprints: command.fingerprint.clone(),
            no_models: args.no_models,
            options: command.options.clone(),
        };
        let done = apply(ctx, &selection, !args.no_store, !args.no_cache)?;
        messages = done.lines();
        shown = done.shown.clone();
        if command.options.dry_run && command.format == CheckFormat::Text {
            return Ok(Reply::text(done.diff).with_messages(&messages));
        }
    }
    let mut checked = analyze(ctx, args)?;
    messages.append(&mut checked.messages);
    checked.messages = messages;
    let reporting = Instant::now();
    let mut reply = match command.format {
        CheckFormat::Json => Reply::json(&checked.report(args.detail.into(), args.limit))?,
        CheckFormat::Sarif => Reply::text(checked.sarif_with_fixes(args.limit, &shown)),
        CheckFormat::Text => Reply::text(checked.render(Format::Text, args.limit)),
    }
    .with_messages(&checked.messages);
    if command.timings {
        reply.notes.extend(
            checked
                .run
                .timings
                .lines(reporting.elapsed(), started.elapsed()),
        );
    }
    reply.code = checked.run.exit_code(
        FailOn {
            strict: args.strict,
            max_warnings: args.max_warnings,
        },
        args.allow_incomplete,
    );
    Ok(reply)
}

/// The `fix` tool: fix, and say what happened.
pub fn fix(ctx: &Context, args: &FixArgs, format: reply::Format) -> Result<Reply> {
    let fixed = apply(ctx, args, true, true)?;
    let reply = Reply::data(format, &fixed, || fixed.diff.clone())?;
    Ok(reply.with_messages(&fixed.lines()))
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
        ..CheckRequest::default()
    };
    lighthouse_session::check(&session, &request)
}

/// Fixes what `args` selects. `store` says whether the run is remembered and
/// `cache` whether the answers of models are kept.
fn apply(ctx: &Context, args: &FixArgs, store: bool, cache: bool) -> Result<Fixed> {
    if ctx.confined()
        && args.fingerprints.is_empty()
        && args.paths.is_empty()
        && args.decision.is_empty()
    {
        return Err(
            "name what to fix: `fingerprints` of findings, `paths`, or `decision`; a fix never defaults to the whole project"
                .into(),
        );
    }
    let session = Session::load(ctx.config.as_deref())?;
    let request = Selection {
        targets: Targets {
            paths: ctx.paths(&session.root, &args.paths)?,
            fingerprints: args.fingerprints.clone(),
            rules: args.decision.clone(),
        },
        dry_run: args.options.dry_run,
        unsafe_fixes: args.options.unsafe_permission(),
        fixer: args.options.fixer.clone(),
        store,
        models: ModelOptions {
            off: args.no_models,
            ..ModelOptions::default()
        },
        cache,
    };
    fix::run(&session, &request)
}
