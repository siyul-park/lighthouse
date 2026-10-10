use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::{
    Comment, Diagnostic, Fingerprint, Position, Project, Severity, Span,
    annotation::{self, ANNOTATION_REASON, Directive, Form, UNUSED_ALLOW},
};
use lighthouse_plugin::RuleManifest;
use lighthouse_spec::ProjectError;
use serde_json::json;

/// A finding that a source directive allows: it is not reported and does not
/// fail the run, but the run still states how many there were. In SARIF terms
/// the directive is a suppression of kind `inSource`.
#[derive(Debug, Clone, PartialEq)]
pub struct Allowed {
    pub diagnostic: Diagnostic,
    /// The reason the directive gives.
    pub reason: String,
}

/// The level the configuration gives a rule at a file of a language.
pub(crate) type LevelAt<'a> =
    dyn Fn(&str, &Path, &str) -> Result<Option<Severity>, ProjectError> + 'a;

/// What the run knows about rules, which directives need to judge whether
/// they are used.
pub(crate) struct Gate<'a> {
    /// The level a rule has at a file and language, `None` when it is off.
    pub level: &'a LevelAt<'a>,
    /// Rules enabled by the configuration.
    pub active: &'a BTreeSet<String>,
    /// Rules that ran in this run.
    pub selected: &'a BTreeSet<String>,
    /// The manifest of each rule that ran: its uid and former names seed the
    /// fingerprints of the findings the engine reports itself.
    pub manifests: &'a BTreeMap<String, &'a RuleManifest>,
    /// Ids decisions were renamed from, with their current ids: a directive
    /// that names an old id still means the decision.
    pub aliases: &'a BTreeMap<String, String>,
    /// The text of a file of the run.
    pub text: &'a dyn Fn(&Path) -> Option<&'a str>,
}

/// What applying the directives came to.
pub(crate) struct Applied {
    /// The findings that remain, and the findings about directives.
    pub kept: Vec<Diagnostic>,
    pub allowed: Vec<Allowed>,
    /// What the user should know: old ids a directive still uses.
    pub notices: BTreeSet<String>,
}

/// A directive on a line of a comment of the file being read.
struct Located<'a> {
    comment: &'a Comment,
    /// The index of the directive's line in the comment's text.
    index: usize,
    /// The directive's line in the file.
    line: u32,
    directive: Directive,
}

/// Which findings a claim reaches.
#[derive(Clone, Copy)]
enum Reach {
    /// The line after the directive and its own line; and the symbol the
    /// comment documents, when the directive is the last line of the comment.
    NextLine(u32),
    /// The directive's own line.
    Line(u32),
    /// From a line to the matching `lighthouse-enable` (exclusive), or to the
    /// end of the file.
    Range { from: u32, to: Option<u32> },
}

/// One rule named by one directive that disables it.
struct Claim<'a> {
    at: &'a Located<'a>,
    /// The decision's current id.
    rule: String,
    reach: Reach,
}

/// The directives of one file read in source order: what each disables, which
/// ranges are open, and the findings about directives that are wrong.
struct Reader<'a, 'g> {
    gate: &'g Gate<'g>,
    lang: &'a str,
    /// The line of the file's first code; a `lighthouse-disable` above it is
    /// at the top of the file.
    first_code: u32,
    claims: Vec<Claim<'a>>,
    /// The claims of the ranges still open, by rule.
    open: BTreeMap<String, Vec<usize>>,
    extra: Vec<Diagnostic>,
    notices: BTreeSet<String>,
}

impl<'a> Reader<'a, '_> {
    fn read(&mut self, at: &'a Located<'a>) -> Result<(), ProjectError> {
        let directive = &at.directive;
        if directive.rules.is_empty() {
            let message = format!(
                "`{}` names no decision; there is no form without ids, name the decisions it is for",
                directive.marker
            );
            return self.note(at, UNUSED_ALLOW, message);
        }
        if directive.form == Form::Enable {
            return self.enable(at);
        }
        if directive.reason.is_none() {
            let marker = directive.marker;
            let message = format!(
                "`{marker}` needs a reason: write `{marker} <rule> -- <why>`; the annotation is ignored"
            );
            return self.note(at, ANNOTATION_REASON, message);
        }
        for id in &directive.rules {
            let rule = self.canonical(id);
            let reach = if directive.form == Form::Line {
                Reach::Line(at.line)
            } else if directive.form == Form::Disable {
                Reach::Range {
                    from: if at.line < self.first_code {
                        1
                    } else {
                        at.line
                    },
                    to: None,
                }
            } else {
                Reach::NextLine(at.line)
            };
            if matches!(reach, Reach::Range { .. }) {
                self.open
                    .entry(rule.clone())
                    .or_default()
                    .push(self.claims.len());
            }
            self.claims.push(Claim { at, rule, reach });
        }
        Ok(())
    }

    /// `lighthouse-enable` ends the open ranges of the rules it names; one
    /// that closes nothing is reported.
    fn enable(&mut self, at: &'a Located<'a>) -> Result<(), ProjectError> {
        for id in &at.directive.rules {
            let rule = self.canonical(id);
            let Some(ranges) = self.open.remove(&rule) else {
                let message = format!(
                    "`{} {rule}` has no matching `lighthouse-disable`; remove it",
                    at.directive.marker
                );
                self.note(at, UNUSED_ALLOW, message)?;
                continue;
            };
            for range in ranges {
                if let Reach::Range { to, .. } = &mut self.claims[range].reach {
                    *to = Some(at.line);
                }
            }
        }
        Ok(())
    }

    /// The decision's current id for an id a directive names.
    fn canonical(&mut self, id: &str) -> String {
        match self.gate.aliases.get(id) {
            Some(new) => {
                self.notices.insert(format!(
                    "decision `{id}` is now `{new}`; a directive still names the old id (`lighthouse spec migrate` rewrites it)"
                ));
                new.clone()
            }
            None => id.to_owned(),
        }
    }

    fn note(&mut self, at: &Located, rule: &str, message: String) -> Result<(), ProjectError> {
        self.extra
            .extend(finding(self.gate, at, self.lang, rule, message)?);
        Ok(())
    }
}

/// Applies the suppression directives of the project to `found`: a finding of
/// a named rule that a directive reaches is removed and returned as allowed,
/// whatever its severity. A directive that disables without a reason is
/// ignored and reported, and so is one that suppresses nothing, an
/// unmatched `lighthouse-enable` and one that names no rule, when the rules
/// for them are enabled.
pub(crate) fn apply<'g>(
    found: Vec<Diagnostic>,
    project: &Project,
    gate: &'g Gate<'g>,
) -> Result<Applied, ProjectError> {
    let mut found: Vec<Option<Diagnostic>> = found.into_iter().map(Some).collect();
    let mut allowed = Vec::new();
    let mut extra = Vec::new();
    let mut notices = BTreeSet::new();
    for file in &project.files {
        let located = locate(project, &file.path);
        if located.is_empty() {
            continue;
        }
        let first_code = (gate.text)(&file.path)
            .map_or(u32::MAX, |text| first_code_line(project, &file.path, text));
        let mut reader = Reader {
            gate,
            lang: &file.lang,
            first_code,
            claims: Vec::new(),
            open: BTreeMap::new(),
            extra: Vec::new(),
            notices: BTreeSet::new(),
        };
        for at in &located {
            reader.read(at)?;
        }
        let Reader {
            claims,
            extra: noted,
            notices: renamed,
            ..
        } = reader;
        extra.extend(noted);
        notices.extend(renamed);
        for claim in &claims {
            let taken = take(&mut found, &file.path, claim);
            extra.extend(unused_note(gate, &file.lang, claim, taken.is_empty())?);
            let reason = claim.at.directive.reason.clone().unwrap_or_default();
            allowed.extend(taken.into_iter().map(|diagnostic| Allowed {
                diagnostic,
                reason: reason.clone(),
            }));
        }
    }
    let mut kept: Vec<Diagnostic> = found.into_iter().flatten().collect();
    kept.extend(extra);
    Ok(Applied {
        kept,
        allowed,
        notices,
    })
}

/// The directives of the comments of `file`, in source order.
fn locate<'a>(project: &'a Project, file: &Path) -> Vec<Located<'a>> {
    project
        .comments_in(file)
        .iter()
        .flat_map(|comment| {
            annotation::directives(&comment.text)
                .into_iter()
                .map(move |(index, directive)| Located {
                    comment,
                    index,
                    line: comment.span.start.line + count(index),
                    directive,
                })
        })
        .collect()
}

/// The finding about a claim that suppressed nothing, when its rule could
/// have fired.
fn unused_note(
    gate: &Gate,
    lang: &str,
    claim: &Claim,
    nothing: bool,
) -> Result<Option<Diagnostic>, ProjectError> {
    if !nothing || !unused(gate, &claim.rule) {
        return Ok(None);
    }
    let marker = claim.at.directive.marker;
    let place = match claim.reach {
        Reach::Range { .. } => "in its range",
        Reach::NextLine(_) | Reach::Line(_) => "here",
    };
    let message = format!(
        "`{marker} {}` suppresses nothing {place}; remove it",
        claim.rule
    );
    finding(gate, claim.at, lang, UNUSED_ALLOW, message)
}

/// Removes and returns the findings of the claim's rule that it reaches.
fn take(found: &mut [Option<Diagnostic>], file: &Path, claim: &Claim) -> Vec<Diagnostic> {
    let mut taken = Vec::new();
    for slot in found.iter_mut() {
        let reaches = slot
            .as_ref()
            .is_some_and(|d| d.rule_id == claim.rule && d.file == file && reaches(claim, d));
        if reaches {
            taken.extend(slot.take());
        }
    }
    taken
}

fn reaches(claim: &Claim, d: &Diagnostic) -> bool {
    let line = d.span.start.line;
    match claim.reach {
        Reach::NextLine(at) => line == at || line == at + 1 || documents(claim, d),
        Reach::Line(at) => line == at,
        Reach::Range { from, to } => line >= from && to.is_none_or(|to| line < to),
    }
}

/// Whether the finding is about the symbol the claim's comment documents: a
/// directive on the last line of its comment reaches the symbol declared
/// below it, doc comments and attributes in between included.
fn documents(claim: &Claim, d: &Diagnostic) -> bool {
    let comment = claim.at.comment;
    claim.at.line == comment.span.end.line
        && comment.file == d.file
        && comment
            .attached_to
            .as_ref()
            .is_some_and(|id| d.symbol.as_deref() == Some(id.as_str()))
}

/// A directive for `rule` is unused when the rule could have fired: it ran,
/// or the configuration does not enable it at all (so it never will).
fn unused(gate: &Gate, rule: &str) -> bool {
    gate.selected.contains(rule) || !gate.active.contains(rule)
}

fn finding(
    gate: &Gate,
    at: &Located,
    lang: &str,
    rule: &str,
    message: String,
) -> Result<Option<Diagnostic>, ProjectError> {
    let comment = at.comment;
    if !gate.selected.contains(rule) {
        return Ok(None);
    }
    let Some(severity) = (gate.level)(rule, &comment.file, lang)? else {
        return Ok(None);
    };
    let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    // The identity does not depend on how a directive is spelled: its marker
    // and the ids it names (old or new) are written one way.
    let canonical = annotation::canonical(&comment.text, &|id| {
        gate.aliases.get(id).map_or(id, String::as_str).to_owned()
    });
    let mut current = words(&canonical);
    if at.index > 0 {
        current.push_str(&format!(" #{}", at.index));
    }
    let raw = words(&comment.text);
    let path = comment.file.to_string_lossy();
    let (fingerprint, legacy) = match gate.manifests.get(rule) {
        Some(meta) => meta.fingerprints_for(&path, &current, &raw),
        None => (Fingerprint::of(rule, &path, &current), Vec::new()),
    };
    let mut d = Diagnostic::new(
        rule,
        severity,
        message,
        &comment.file,
        place(at),
        fingerprint,
    )
    .with_legacy(legacy);
    d.evidence = json!({
        "rules": at.directive.rules,
        "reason": at.directive.reason,
        "annotation": annotation_span(at),
    });
    Ok(Some(d))
}

/// Where a finding about the directive is reported: the comment, or, for a
/// directive on a later line of a comment, that line.
fn place(at: &Located) -> Span {
    if at.index == 0 {
        return at.comment.span;
    }
    let line = at.comment.text.lines().nth(at.index).unwrap_or_default();
    let width = count(line.trim_end().len());
    Span {
        start: Position {
            line: at.line,
            col: 1,
        },
        end: Position {
            line: at.line,
            col: 1 + width,
        },
    }
}

/// The span of the comment's line that holds the directive: from the
/// comment's own start on its first line, the whole line (newline included)
/// on a later one. A fix that removes it leaves the rest of the comment.
fn annotation_span(at: &Located) -> Span {
    let comment = at.comment;
    let number = at.line;
    if at.index > 0 {
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
    let width = count(first.trim_end_matches('\r').len());
    Span {
        start: comment.span.start,
        end: Position {
            line: number,
            col: comment.span.start.col + width,
        },
    }
}

/// The line of the first thing in a file that is neither blank nor comment: a
/// `lighthouse-disable` above it is at the top of the file. Imports are code.
fn first_code_line(project: &Project, file: &Path, text: &str) -> u32 {
    let comments = project.comments_in(file);
    let covered = |line: u32, content: &str| {
        comments.iter().any(|c| {
            let (start, end) = (c.span.start, c.span.end);
            let before = (content.len()).min(start.col.saturating_sub(1) as usize);
            let standalone = start.line < line || content[..before].trim().is_empty();
            line >= start.line && line <= end.line && standalone
        })
    };
    text.lines()
        .enumerate()
        .map(|(at, content)| (count(at) + 1, content))
        .find(|(line, content)| !content.trim().is_empty() && !covered(*line, content))
        .map_or(u32::MAX, |(line, _)| line)
}

/// A line count as the `u32` positions are in; a comment is never that long.
fn count(n: usize) -> u32 {
    u32::try_from(n).expect("a comment has fewer lines than a u32 position can number")
}
