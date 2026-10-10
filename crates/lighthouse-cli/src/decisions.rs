use std::path::Path;

use lighthouse_session::{DecisionRow, Session, bundled_decision_rows, explain_bundled};

use crate::Result;

/// Runs the examples of the checked decisions named by `ids` (all when
/// empty) in every language the project's plugins provide, or in `language`.
/// Returns 1 when an example fails.
pub fn test(ids: &[String], language: Option<&str>, config: Option<&Path>) -> Result<u8> {
    let session = Session::load_or_default(config)?;
    let report = lighthouse_session::test_decisions(&session, ids, language)?;
    for failure in &report.failures {
        println!("FAIL {failure}");
    }
    println!(
        "decision test: {} decision(s) in {} language(s), {} run(s), {} failure(s)",
        report.decisions,
        report.languages,
        report.runs,
        report.failures.len()
    );
    Ok(u8::from(!report.failures.is_empty()))
}

pub fn list(all: bool) -> Result<u8> {
    for DecisionRow {
        id,
        status,
        severity,
        title,
    } in bundled_decision_rows(all)
    {
        if all {
            println!("{id}\t{status}\t{severity}\t{title}");
        } else {
            println!("{id}\t{severity}\t{title}");
        }
    }
    Ok(0)
}

pub fn explain(id: &str) -> Result<u8> {
    print!("{}", explain_bundled(id)?);
    Ok(0)
}
