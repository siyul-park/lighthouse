//! `lighthouse check`: select what to look at, optionally fix it, run the
//! analysis and report the findings.

use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use lighthouse_report::{Briefing, Detail, Format, render_with};
use lighthouse_session::{CheckRequest, Checked, FailOn, FixSelection, FixTargets, Session};

use crate::{Result, SARIF_FIXES, timings};

/// Which findings a check looks at, and whether it records the run.
pub struct Scope<'a> {
    pub paths: Vec<PathBuf>,
    pub reported: Reported<'a>,
    pub only: &'a [String],
    pub config: Option<&'a Path>,
    pub store: bool,
    pub cache: bool,
}

/// How a check reports.
pub struct Output {
    pub format: Format,
    pub limit: Option<usize>,
    pub detail: Detail,
    pub fail_on: FailOn,
    pub allow_incomplete: bool,
    pub timings: bool,
}

/// What `check --fix` is asked to do.
pub struct FixOptions {
    pub dry_run: bool,
    pub unsafe_fixes: bool,
    pub fixer: Option<String>,
}

/// A report filter taken from git instead of paths.
pub struct Reported<'a> {
    pub changed: bool,
    pub diff: Option<&'a str>,
}

/// Checks the scope, fixing first when asked, and reports. The exit code says
/// whether the findings fail the run.
pub fn run(scope: &Scope, output: &Output, fix: Option<FixOptions>) -> Result<u8> {
    let started = Instant::now();
    validate(output)?;
    if let Some(fix) = fix {
        let dry_run = fix.dry_run;
        fixing(scope, fix)?;
        if dry_run {
            return Ok(0);
        }
    }
    let checked = select(scope)?;
    report(&checked, output, started)
}

/// Refuses the flags that only the agent formats honor.
fn validate(output: &Output) -> Result<()> {
    let agent = matches!(output.format, Format::Agent | Format::AgentJson);
    if output.limit.is_some() && !agent {
        return Err("--limit applies to the agent formats only".into());
    }
    if output.detail != Detail::default() && !agent {
        return Err("--detail applies to the agent formats only".into());
    }
    Ok(())
}

/// Analyzes the project and keeps what the scope reports.
fn select(scope: &Scope) -> Result<Checked> {
    let request = CheckRequest {
        paths: scope.paths.clone(),
        changed: scope.reported.changed,
        diff: scope.reported.diff.map(str::to_owned),
        rules: scope.only.to_vec(),
        store: scope.store,
        cache: scope.cache,
    };
    let checked = lighthouse_session::check(Session::load(scope.config)?, &request)?;
    for message in &checked.messages {
        eprintln!("lighthouse: {message}");
    }
    Ok(checked)
}

/// Prints the findings in the format asked for and gives the exit code.
fn report(checked: &Checked, output: &Output, started: Instant) -> Result<u8> {
    let outcome = &checked.outcome;
    let fixes = match output.format {
        Format::Agent | Format::AgentJson => checked.shown_fixes(output.limit, output.detail),
        Format::Sarif => {
            let all: Vec<_> = outcome.diagnostics.iter().take(SARIF_FIXES).collect();
            checked.fixes(&all)
        }
        Format::Text | Format::Json => BTreeMap::new(),
    };
    let reporting = Instant::now();
    let sources = if output.format == Format::Sarif {
        checked.sources()
    } else {
        BTreeMap::new()
    };
    print!(
        "{}",
        render_with(
            output.format,
            &outcome.diagnostics,
            &outcome.incomplete,
            &Briefing {
                detail: output.detail,
                fixes: Some(&fixes),
                sources: Some(&sources),
                ..checked.briefing(output.limit)
            }
        )
    );
    if output.timings {
        for line in timings::lines(&outcome.timings, reporting.elapsed(), started.elapsed()) {
            writeln!(io::stderr(), "{line}").ok();
        }
    }
    Ok(outcome.exit_code(output.fail_on, output.allow_incomplete))
}

/// Fixes the selected findings and says what happened: the diff of a dry run
/// on stdout, everything else on stderr.
fn fixing(scope: &Scope, fix: FixOptions) -> Result<()> {
    let request = FixSelection {
        targets: FixTargets {
            paths: scope.paths.clone(),
            rules: scope.only.to_vec(),
            ..FixTargets::default()
        },
        dry_run: fix.dry_run,
        unsafe_fixes: fix.unsafe_fixes,
        fixer: fix.fixer,
        store: scope.store,
    };
    let fixed = lighthouse_session::fix(Session::load(scope.config)?, &request)?;
    for message in &fixed.messages {
        eprintln!("lighthouse: {message}");
    }
    if fixed.dry_run {
        print!("{}", fixed.diff);
    }
    let verb = if fixed.dry_run { "would fix" } else { "fixed" };
    for fix in fixed.applied() {
        eprintln!(
            "{verb} {} [{}] {} ({}): {}",
            fix.rule,
            fix.safety,
            fix.files.join(", "),
            fix.fixer,
            fix.description
        );
    }
    for fix in fixed.declined() {
        eprintln!(
            "not fixed {}:{} {}: {}",
            fix.path, fix.line, fix.rule, fix.reason
        );
    }
    eprintln!(
        "lighthouse: {} fix(es) {}, {} left alone",
        fixed.applied().len(),
        if fixed.dry_run { "proposed" } else { "applied" },
        fixed.declined().len()
    );
    Ok(())
}
