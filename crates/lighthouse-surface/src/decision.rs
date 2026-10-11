//! `decision list|show|create|update|test`.

use std::{
    fmt::Write,
    fs,
    io::{Read, stdin},
};

use lighthouse_session::{
    Authored, DecisionRow, Session, active_decisions, create_decision, decision_rows, explain,
    replace_decision, test_decisions,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    Result,
    args::{DecisionListArgs, DecisionShowArgs, DecisionTestArgs},
    list::{Page, omitted_line, page},
    reply::{Context, FOUND, Output, Reply},
};

/// The `DecisionList` kind: the decisions with a rule, or every one.
#[derive(Debug, Serialize)]
pub struct DecisionList {
    pub decisions: Vec<DecisionItem>,
    /// How many decisions `limit` left out.
    pub omitted: usize,
}

/// One decision of a listing.
#[derive(Debug, Serialize)]
pub struct DecisionItem {
    #[serde(flatten)]
    pub row: DecisionRow,
    /// Whether this project's configuration enables it.
    pub enabled: bool,
}

/// The `DecisionShow` kind.
#[derive(Debug, Serialize)]
pub struct DecisionShow {
    pub id: String,
    /// Context, requirement, examples and status.
    pub markdown: String,
}

/// The `TestReport` kind: the examples of the checked decisions, run.
#[derive(Debug, Serialize)]
pub struct TestReport {
    /// False when any example fails.
    pub ok: bool,
    pub decisions: usize,
    pub languages: usize,
    pub runs: usize,
    /// One line per failing example.
    pub failures: Vec<String>,
}

/// The `AuthorReport` kind: a decision that was written.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorReport {
    pub id: String,
    pub path: String,
    pub created: bool,
    pub tested: Tested,
    pub local_plugin_listed: bool,
    /// What to do next.
    pub note: String,
}

/// What the gate ran before the decision was written.
#[derive(Debug, Serialize)]
pub struct Tested {
    pub runs: usize,
    pub languages: usize,
}

/// `decision list`.
pub fn list(ctx: &Context, args: &DecisionListArgs, output: Output) -> Result<Reply> {
    let session = Session::load_or_default(ctx.config.as_deref())?;
    let catalog = session.catalog()?;
    let registry = session.in_process_registry()?;
    let enabled = active_decisions(&session)?;
    let rows = decision_rows(&catalog, &registry, args.all);
    let Page { items, omitted } = page(rows, args.limit);
    let decisions: Vec<DecisionItem> = items
        .into_iter()
        .map(|row| DecisionItem {
            enabled: enabled.contains(&row.id),
            row,
        })
        .collect();
    let all = args.all;
    let list = DecisionList { decisions, omitted };
    Reply::data(output, &list, || {
        let mut out = String::new();
        for DecisionItem { row, .. } in &list.decisions {
            let _ = if all {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}",
                    row.id, row.status, row.severity, row.title
                )
            } else {
                writeln!(out, "{}\t{}\t{}", row.id, row.severity, row.title)
            };
        }
        out.push_str(&omitted_line(omitted));
        out
    })
}

/// `decision show`.
pub fn show(ctx: &Context, args: &DecisionShowArgs, output: Output) -> Result<Reply> {
    let session = Session::load_or_default(ctx.config.as_deref())?;
    let catalog = session.catalog()?;
    let registry = session.in_process_registry()?;
    let markdown = explain(&catalog, &registry, &args.id)?;
    let shown = DecisionShow {
        id: args.id.clone(),
        markdown: markdown.clone(),
    };
    Reply::data(output, &shown, || markdown)
}

/// `decision create` and `decision update`: the document is YAML or JSON text.
pub fn author(ctx: &Context, text: &str, create: bool, output: Output) -> Result<Reply> {
    let document: Value = serde_norway::from_str(text)
        .map_err(|e| format!("the document is not valid YAML or JSON: {e}"))?;
    let session = Session::load(ctx.config.as_deref())?;
    let done = if create {
        create_decision(session, document)?
    } else {
        replace_decision(session, document)?
    };
    let report = authored(&done);
    let verb = if done.created { "created" } else { "updated" };
    Reply::data(output, &report, || {
        format!("{verb} {} at {}\n{}\n", report.id, report.path, report.note)
    })
}

/// The text of the document a command line names: a file, or `-` for stdin.
pub fn read_document(source: &str) -> Result<String> {
    if source == "-" {
        let mut text = String::new();
        stdin().read_to_string(&mut text)?;
        return Ok(text);
    }
    fs::read_to_string(source).map_err(|e| format!("{source}: {e}").into())
}

/// `decision test`.
pub fn test(ctx: &Context, args: &DecisionTestArgs, output: Output) -> Result<Reply> {
    let session = Session::load_or_default(ctx.config.as_deref())?;
    let ran = test_decisions(&session, &args.decision, args.language.as_deref())?;
    let report = TestReport {
        ok: ran.failures.is_empty(),
        decisions: ran.decisions,
        languages: ran.languages,
        runs: ran.runs,
        failures: ran.failures,
    };
    let code = if report.ok { 0 } else { FOUND };
    let reply = Reply::data(output, &report, || {
        let mut out = String::new();
        for failure in &report.failures {
            let _ = writeln!(out, "FAIL {failure}");
        }
        let _ = writeln!(
            out,
            "decision test: {} decision(s) in {} language(s), {} run(s), {} failure(s)",
            report.decisions,
            report.languages,
            report.runs,
            report.failures.len()
        );
        out
    })?;
    Ok(reply.with_code(code))
}

fn authored(done: &Authored) -> AuthorReport {
    let note = if done.plugin_listed {
        "the `local` plugin is listed; enable the decision with `extends = [\"local/recommended\"]` or a [rules] entry if it is not on yet"
    } else {
        "add \"local\" to `plugins` in lighthouse.toml, and enable the decision, for `check` to run it"
    };
    AuthorReport {
        id: done.id.clone(),
        path: done.path.display().to_string(),
        created: done.created,
        tested: Tested {
            runs: done.test.runs,
            languages: done.test.languages,
        },
        local_plugin_listed: done.plugin_listed,
        note: note.to_owned(),
    }
}
