//! `output: sarif`: the findings of a SARIF 2.1.0 log a program printed. One
//! result is one finding, placed in the project, with the severity the
//! decision authored; the tool's own level, rule and help link are evidence.
//! A result the tool suppressed in the code is reported as an `inSource`
//! suppression, never dropped.

mod log;
mod place;
mod words;

use std::path::{Path, PathBuf};

use lighthouse_model::{ColumnUnit, Diagnostic, Position, Span, byte_position};
use lighthouse_plugin::{Ctx, RuleManifest};
use lighthouse_spec::SarifSelect;
use serde_json::{Map, Value, json};

use self::log::{Log, Run, SarifResult, VERSION};
use crate::glob;

/// What a result with no `level` has, in SARIF.
const DEFAULT_LEVEL: &str = "warning";

/// A SARIF log being read for one rule.
pub(super) struct Reading<'a> {
    pub ctx: &'a Ctx<'a>,
    pub meta: &'a RuleManifest,
    pub select: Option<&'a SarifSelect>,
    /// The exit code said the program found something.
    pub found: bool,
}

/// Where a result lies.
enum Place {
    /// Nowhere: it is about the project.
    Project,
    /// In a file the project does not have.
    Outside,
    File(PathBuf, Span),
}

impl Reading<'_> {
    /// The findings of the log on `stdout`. A log that cannot be read, or an
    /// exit code that says it found something over a log with no result, is
    /// an error: the analysis is incomplete, never clean.
    pub(super) fn diagnostics(&self, stdout: &str) -> Result<Vec<Diagnostic>, String> {
        if stdout.trim().is_empty() {
            return if self.found {
                Err(
                    "exited with a code that says it found something, but printed no SARIF log"
                        .to_owned(),
                )
            } else {
                Ok(Vec::new())
            };
        }
        let log: Log = serde_json::from_str(stdout)
            .map_err(|e| format!("printed output that is not a SARIF log: {e}"))?;
        if log.version != VERSION {
            return Err(format!(
                "printed a SARIF log of version `{}`; only {VERSION} is read",
                log.version
            ));
        }
        if self.found && log.runs.iter().all(|r| r.results.is_empty()) {
            return Err(
                "exited with a code that says it found something, but its SARIF log has no result"
                    .to_owned(),
            );
        }
        let mut found = Vec::new();
        for run in &log.runs {
            for result in run.results.iter().filter(|r| self.selected(run, r)) {
                found.extend(self.diagnostic(run, result));
            }
        }
        Ok(found)
    }

    /// Whether the decision's `select` lets the result through.
    fn selected(&self, run: &Run, result: &SarifResult) -> bool {
        let Some(select) = self.select else {
            return true;
        };
        let id = rule_id(run, result);
        let level = result.level.as_deref().unwrap_or(DEFAULT_LEVEL);
        let by_rule =
            select.rule_ids.is_empty() || select.rule_ids.iter().any(|g| glob::matches(&id, g));
        let by_level = select.levels.is_empty() || select.levels.iter().any(|l| l == level);
        by_rule && by_level
    }

    /// The finding for one result; `None` for a result outside the project.
    fn diagnostic(&self, run: &Run, result: &SarifResult) -> Option<Diagnostic> {
        let (path, span) = match self.place(run, result) {
            Place::Outside => return None,
            Place::Project => (PathBuf::from("."), at(Position { line: 1, col: 1 })),
            Place::File(path, span) => (path, span),
        };
        let id = rule_id(run, result);
        let rule = run.rule(result);
        let text = words::message(&result.message, rule, &id);
        let subject = self.subject(&path, span.start);
        let snippet = format!("{id} {}", words::identifying(&text));
        let mut diagnostic = Diagnostic::new(
            &self.meta.id,
            self.meta.severity,
            text,
            &path,
            span,
            self.meta.fingerprint(&subject, &snippet),
        );
        diagnostic.evidence = evidence(run, result, &id, rule.and_then(|r| r.help_uri.as_deref()));
        let reasons: Vec<&str> = result
            .suppressions
            .iter()
            .filter(|s| s.in_force())
            .map(|s| s.justification.as_deref().unwrap_or_default())
            .collect();
        if reasons.is_empty() {
            return Some(diagnostic);
        }
        Some(diagnostic.suppressed_by_tool(&reasons.join("; ")))
    }

    fn place(&self, run: &Run, result: &SarifResult) -> Place {
        let physical = result
            .locations
            .first()
            .and_then(|l| l.physical_location.as_ref());
        let Some(location) = physical.and_then(|p| p.artifact_location.as_ref()) else {
            return Place::Project;
        };
        let Some(path) = place::path_of(location, &run.original_uri_base_ids) else {
            return Place::Project;
        };
        let path = place::relative(&path, &self.ctx.ws.root);
        if self.ctx.project.file(&path).is_none() {
            return Place::Outside;
        }
        let region = physical.and_then(|p| p.region.as_ref());
        let start = Position {
            line: region.and_then(|r| r.start_line).unwrap_or(1).max(1),
            col: region.and_then(|r| r.start_column).unwrap_or(1).max(1),
        };
        let end = region
            .and_then(|r| {
                r.end_column
                    .map(|col| (r.end_line.unwrap_or(start.line), col))
            })
            .map_or(start, |(line, col)| Position { line, col });
        let unit = match run.column_kind.as_deref() {
            Some("unicodeCodePoints") => ColumnUnit::CodePoints,
            _ => ColumnUnit::Utf16,
        };
        let text = super::read(self.ctx, &path).ok();
        let bytes = |p: Position| text.as_ref().map_or(p, |t| byte_position(t, p, unit));
        let span = Span {
            start: bytes(start),
            end: bytes(end),
        };
        Place::File(path, span)
    }

    /// What the finding is about, for its identity: the innermost symbol of
    /// the project that contains the start, else the file. Where the line is
    /// does not identify it, so moving code keeps the finding.
    fn subject(&self, path: &Path, start: Position) -> String {
        self.ctx
            .project
            .symbols_in(path)
            .filter(|s| {
                let extent = s.extent.unwrap_or(s.span);
                extent.start <= start && start <= extent.end
            })
            .max_by_key(|s| s.extent.unwrap_or(s.span).start)
            .map_or_else(|| super::slashed(path), |s| s.id.as_str().to_owned())
    }
}

fn at(position: Position) -> Span {
    Span {
        start: position,
        end: position,
    }
}

/// The rule of a result: its `ruleId`, else the id of the rule `ruleIndex`
/// points at.
fn rule_id(run: &Run, result: &SarifResult) -> String {
    result
        .rule_id
        .clone()
        .or_else(|| run.rule(result).map(|r| r.id.clone()))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn evidence(run: &Run, result: &SarifResult, id: &str, help: Option<&str>) -> Value {
    let mut map = Map::new();
    map.insert("tool".to_owned(), json!(run.tool.driver.name));
    map.insert("ruleId".to_owned(), json!(id));
    map.insert(
        "level".to_owned(),
        json!(result.level.as_deref().unwrap_or(DEFAULT_LEVEL)),
    );
    if let Some(help) = help {
        map.insert("helpUri".to_owned(), json!(help));
    }
    Value::Object(map)
}
