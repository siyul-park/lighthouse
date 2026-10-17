//! `model rebuild`: the artifacts of attached models a machine does not have,
//! made again from their labels.

use std::fmt::Write;

use lighthouse_session::model::{RebuildRequest, Rebuilt, rebuild};

use crate::surface::{
    Result,
    args::ModelRebuildArgs,
    eval::{default_baseline, labelling_of},
    reply::{Context, FOUND, Output, Reply},
};

/// `model rebuild`: exits 1 unless every model is present or made again.
pub fn run(ctx: &Context, args: &ModelRebuildArgs, output: Output) -> Result<Reply> {
    let baseline = args.baseline.clone().or_else(|| match args.ids.as_slice() {
        [one] => default_baseline(one),
        _ => None,
    });
    let rows = rebuild(&RebuildRequest {
        decisions: args.ids.clone(),
        labelling: labelling_of(ctx, &args.repo, baseline)?,
    })?;
    let holds = rows.iter().all(|row| row.status.holds());
    let reply = Reply::data(output, &rows, || text(&rows))?;
    Ok(if holds { reply } else { reply.with_code(FOUND) })
}

fn text(rows: &[Rebuilt]) -> String {
    let mut out = String::new();
    for row in rows {
        let status = serde_json::to_value(row.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let _ = write!(out, "{}\t{}\t{status}", row.decision, row.model);
        if let Some(detail) = &row.detail {
            let _ = write!(out, "\t{detail}");
        }
        out.push('\n');
    }
    out
}
