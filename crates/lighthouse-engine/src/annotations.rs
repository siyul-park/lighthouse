use std::{collections::BTreeSet, path::Path};

use lighthouse_model::{
    Comment, Diagnostic, Fingerprint, Position, Project, Severity, Span,
    annotation::{self, ANNOTATION_REASON, Allow, UNUSED_ALLOW},
};
use lighthouse_spec::ProjectError;
use serde_json::json;

/// A finding that a source annotation allows: it is not reported and does not
/// fail the run, but the run still states how many there were.
#[derive(Debug, Clone, PartialEq)]
pub struct Allowed {
    pub diagnostic: Diagnostic,
    /// The reason the annotation gives.
    pub reason: String,
}

/// The level the configuration gives a rule at a file of a language.
pub(crate) type LevelAt<'a> =
    dyn Fn(&str, &Path, &str) -> Result<Option<Severity>, ProjectError> + 'a;

/// What the run knows about rules, which annotations need to judge whether
/// they are used.
pub(crate) struct Gate<'a> {
    /// The level a rule has at a file and language, `None` when it is off.
    pub level: &'a LevelAt<'a>,
    /// Rules enabled by the configuration.
    pub active: &'a BTreeSet<String>,
    /// Rules that ran in this run.
    pub selected: &'a BTreeSet<String>,
}

/// Applies the allow annotations of the project to `found`: a finding of a
/// named rule on the annotated symbol or line is removed and returned as
/// allowed, whatever its severity. Annotations without a reason and
/// annotations that suppress nothing become findings of their own, when the
/// rules for them are enabled.
pub(crate) fn apply(
    found: Vec<Diagnostic>,
    project: &Project,
    gate: &Gate,
) -> Result<(Vec<Diagnostic>, Vec<Allowed>), ProjectError> {
    let mut found: Vec<Option<Diagnostic>> = found.into_iter().map(Some).collect();
    let mut allowed = Vec::new();
    let mut extra = Vec::new();
    for file in &project.files {
        for comment in project.comments_in(&file.path) {
            let Some(allow) = annotation::parse(&comment.text) else {
                continue;
            };
            let note = |rule: &str, message: String, allow: &Allow| {
                finding(gate, comment, &file.lang, rule, message, allow)
            };
            let Some(reason) = allow.reason.clone() else {
                extra.extend(note(
                    ANNOTATION_REASON,
                    "`lighthouse:allow` needs a reason: write `lighthouse:allow <rule> -- <why>`; the annotation is ignored".to_owned(),
                    &allow,
                )?);
                continue;
            };
            for rule in &allow.rules {
                let taken = take(&mut found, comment, rule);
                if taken.is_empty() && unused(gate, rule) {
                    extra.extend(note(
                        UNUSED_ALLOW,
                        format!("`lighthouse:allow {rule}` suppresses nothing here; remove it"),
                        &allow,
                    )?);
                }
                allowed.extend(taken.into_iter().map(|diagnostic| Allowed {
                    diagnostic,
                    reason: reason.clone(),
                }));
            }
        }
    }
    let mut kept: Vec<Diagnostic> = found.into_iter().flatten().collect();
    kept.extend(extra);
    Ok((kept, allowed))
}

/// Removes and returns the findings of `rule` that `comment` is attached to.
fn take(found: &mut [Option<Diagnostic>], comment: &Comment, rule: &str) -> Vec<Diagnostic> {
    let mut taken = Vec::new();
    for slot in found.iter_mut() {
        let covers = slot
            .as_ref()
            .is_some_and(|d| d.rule_id == rule && covers(comment, d));
        if covers {
            taken.extend(slot.take());
        }
    }
    taken
}

/// Whether the comment documents the finding's symbol or sits on the finding's
/// line or the line before it.
fn covers(comment: &Comment, d: &Diagnostic) -> bool {
    if comment.file != d.file {
        return false;
    }
    let attached = comment
        .attached_to
        .as_ref()
        .is_some_and(|id| d.symbol.as_deref() == Some(id.as_str()));
    let line = d.span.start.line;
    attached || comment.span.start.line == line || comment.span.end.line + 1 == line
}

/// An annotation for `rule` is unused when the rule could have fired: it ran,
/// or the configuration does not enable it at all (so it never will).
fn unused(gate: &Gate, rule: &str) -> bool {
    gate.selected.contains(rule) || !gate.active.contains(rule)
}

fn finding(
    gate: &Gate,
    comment: &Comment,
    lang: &str,
    rule: &str,
    message: String,
    allow: &Allow,
) -> Result<Option<Diagnostic>, ProjectError> {
    if !gate.selected.contains(rule) {
        return Ok(None);
    }
    let Some(severity) = (gate.level)(rule, &comment.file, lang)? else {
        return Ok(None);
    };
    let text: String = comment
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut d = Diagnostic::new(
        rule,
        severity,
        message,
        &comment.file,
        comment.span,
        Fingerprint::of(rule, &comment.file.to_string_lossy(), &text),
    );
    d.evidence = json!({
        "rules": allow.rules,
        "reason": allow.reason,
        "annotation": annotation_span(comment),
    });
    Ok(Some(d))
}

/// The span of the comment's line that holds the annotation: from the
/// comment's own start on its first line, the whole line (newline included)
/// on a later one. A fix that removes it leaves the rest of the comment.
fn annotation_span(comment: &Comment) -> Span {
    let at = annotation::line_of(&comment.text).unwrap_or(0);
    let number = comment.span.start.line + u32::try_from(at).unwrap_or(0);
    if at > 0 {
        return Span {
            start: Position {
                line: number,
                col: 1,
            },
            end: Position {
                line: number + 1,
                col: 1,
            },
        };
    }
    let first = comment.text.lines().next().unwrap_or_default();
    let width = u32::try_from(first.trim_end_matches('\r').len()).unwrap_or(0);
    Span {
        start: comment.span.start,
        end: Position {
            line: number,
            col: comment.span.start.col + width,
        },
    }
}
