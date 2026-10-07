use std::path::Path;

use lighthouse_session::{RuleRow, Session};
use lighthouse_spec::Catalog;

use crate::Result;

/// Runs the examples of the implemented patterns named by `ids` (all when
/// empty) in every language the project's plugins provide, or in `language`.
/// Returns 1 when an example fails.
pub fn test(ids: &[String], language: Option<&str>, config: Option<&Path>) -> Result<u8> {
    let session = Session::load_or_default(config)?;
    let report = lighthouse_session::test_rules(&session, ids, language)?;
    for failure in &report.failures {
        println!("FAIL {failure}");
    }
    println!(
        "rule test: {} pattern(s) in {} language(s), {} run(s), {} failure(s)",
        report.patterns,
        report.languages,
        report.runs,
        report.failures.len()
    );
    Ok(u8::from(!report.failures.is_empty()))
}

pub fn list(all: bool) -> Result<u8> {
    let registry = lighthouse_builtin::registry();
    for RuleRow {
        id,
        status,
        severity,
        title,
    } in lighthouse_session::rule_rows(Catalog::bundled(), &registry, all)
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
    let registry = lighthouse_builtin::registry();
    print!(
        "{}",
        lighthouse_session::explain(Catalog::bundled(), &registry, id)?
    );
    Ok(0)
}
