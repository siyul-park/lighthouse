//! The `fix` tool: apply the fixes the catalog names, verified.

use std::path::PathBuf;

use lighthouse_session::{FixSelection, Session};
use serde::Deserialize;
use serde_json::json;

use crate::tools::{Outcome, fail, inside};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FixArgs {
    #[serde(default)]
    fingerprints: Vec<String>,
    #[serde(default)]
    paths: Vec<PathBuf>,
    #[serde(default)]
    rules: Vec<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    unsafe_fixes: bool,
}

pub fn fix(args: FixArgs) -> Outcome {
    if args.fingerprints.is_empty() && args.paths.is_empty() && args.rules.is_empty() {
        return Err(
            "name what to fix: `fingerprints` of findings, `paths`, or `rules`; a fix never defaults to the whole project"
                .into(),
        );
    }
    let session = Session::load(None).map_err(fail)?;
    let request = FixSelection {
        paths: inside(&session.root, args.paths)?,
        fingerprints: args.fingerprints,
        rules: args.rules,
        dry_run: args.dry_run,
        unsafe_fixes: args.unsafe_fixes,
        fixer: None,
        store: true,
    };
    let fixed = lighthouse_session::fix(session, &request).map_err(fail)?;
    Ok(json!({
        "dryRun": fixed.dry_run,
        "diff": fixed.diff,
        "applied": fixed.applied(),
        "declined": fixed.declined(),
        "rounds": fixed.report.rounds,
        "messages": fixed.messages,
    }))
}
