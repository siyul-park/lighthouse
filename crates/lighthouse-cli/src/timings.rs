//! `check --timings`: where the time of a run went, as plain lines.

use std::time::Duration;

use lighthouse_session::Timings;

/// How many rules the breakdown names, longest first.
const TOP_RULES: usize = 5;

/// The lines of the breakdown. `report` is the time spent printing the
/// findings and `total` the time of the whole run.
pub fn lines(timings: &Timings, report: Duration, total: Duration) -> Vec<String> {
    let mut out = vec![line("setup", timings.setup), line("read", timings.read)];
    out.extend(
        timings
            .index
            .iter()
            .map(|(language, spent)| line(&format!("index {language}"), *spent)),
    );
    out.push(line("merge", timings.merge));
    if timings.hashing > Duration::ZERO {
        out.push(format!(
            "{} ({} cache hit(s), {} miss(es))",
            line("hashing", timings.hashing),
            timings.cache_hits,
            timings.cache_misses
        ));
    }
    out.extend(
        timings
            .analyzers
            .iter()
            .map(|(analyzer, spent)| line(&format!("analyzer {analyzer}"), *spent)),
    );
    out.push(format!(
        "{} (wall clock)",
        line("rules", timings.rules_wall)
    ));
    out.extend(timings.rules.iter().take(TOP_RULES).map(|(rule, spent)| {
        format!(
            "{} (summed over files)",
            line(&format!("rule {rule}"), *spent)
        )
    }));
    out.push(line("identity", timings.identity));
    out.push(line("store", timings.store));
    out.push(line("report", report));
    out.push(line("total", total));
    out
}

fn line(phase: &str, spent: Duration) -> String {
    format!("timings: {phase} {:.1}ms", spent.as_secs_f64() * 1000.0)
}
