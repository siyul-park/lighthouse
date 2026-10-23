use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use lighthouse_model::{
    Diagnostic, Position, Span, Suppressed, Suppression,
    annotation::{self, Directive, Form},
    squash,
    ucm::{Comment, Project},
};
use serde_json::{Value, json};

/// What the run knows about rules, which directives need to judge whether
/// they are used.
pub(crate) struct Gate<'a> {
    /// Rules enabled by the configuration.
    pub active: &'a BTreeSet<String>,
    /// Rules that ran in this run.
    pub selected: &'a BTreeSet<String>,
    /// The text of a file of the run.
    pub text: &'a dyn Fn(&Path) -> Option<&'a str>,
}

/// What applying the directives came to.
pub(crate) struct Applied {
    /// The findings that remain.
    pub kept: Vec<Diagnostic>,
    pub allowed: Vec<Suppressed>,
    /// What the directives of each file with one say, for the rules that
    /// judge them: one value per rule a directive names (one for a directive
    /// that names none), as [`Reader::annotation`] describes it.
    pub annotations: BTreeMap<PathBuf, Vec<Value>>,
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
enum Coverage {
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
    coverage: Coverage,
}

/// The directives of one file read in source order: what each disables, which
/// ranges are open, and the findings about directives that are wrong.
struct Reader<'a> {
    /// The line of the file's first code; a `lighthouse-disable` above it is
    /// at the top of the file.
    first_code: u32,
    claims: Vec<Claim<'a>>,
    /// The claims of the ranges still open, by rule.
    open: BTreeMap<String, Vec<usize>>,
    /// What the directives that are wrong say, with their claims' own below.
    annotations: Vec<Value>,
}

impl<'a> Reader<'a> {
    fn read(&mut self, at: &'a Located<'a>) {
        let directive = &at.directive;
        if directive.rules.is_empty() {
            self.annotations.push(annotation(at, "", None));
            return;
        }
        if directive.form == Form::Enable {
            self.enable(at);
            return;
        }
        if directive.reason.is_none() {
            // Ignored: it claims nothing, so it is neither used nor unused.
            for rule in &directive.rules {
                self.annotations.push(annotation(at, rule, None));
            }
            return;
        }
        for id in &directive.rules {
            let rule = id.clone();
            let coverage = if directive.form == Form::Line {
                Coverage::Line(at.line)
            } else if directive.form == Form::Disable {
                Coverage::Range {
                    from: if at.line < self.first_code {
                        1
                    } else {
                        at.line
                    },
                    to: None,
                }
            } else {
                Coverage::NextLine(at.line)
            };
            if matches!(coverage, Coverage::Range { .. }) {
                self.open
                    .entry(rule.clone())
                    .or_default()
                    .push(self.claims.len());
            }
            self.claims.push(Claim { at, rule, coverage });
        }
    }

    /// `lighthouse-enable` ends the open ranges of the rules it names; one
    /// that closes nothing is reported.
    fn enable(&mut self, at: &'a Located<'a>) {
        for id in &at.directive.rules {
            let rule = id.clone();
            let closed = self.open.remove(&rule);
            self.annotations
                .push(annotation(at, &rule, Some(closed.is_some())));
            for range in closed.into_iter().flatten() {
                if let Coverage::Range { to, .. } = &mut self.claims[range].coverage {
                    *to = Some(at.line);
                }
            }
        }
    }
}

/// Applies the suppression directives of the project to `found`: a finding of
/// a named rule that a directive reaches is removed and returned as allowed,
/// whatever its severity. A directive that disables without a reason is
/// ignored. What each directive said and whether it suppressed anything is
/// returned for the rules that judge directives.
pub(crate) fn apply<'g>(found: Vec<Diagnostic>, project: &Project, gate: &'g Gate<'g>) -> Applied {
    let mut allowed = Vec::new();
    let mut open = Vec::new();
    for mut diagnostic in found {
        match diagnostic.suppression.take() {
            Some(suppression) => allowed.push(Suppressed {
                diagnostic,
                suppression,
            }),
            None => open.push(diagnostic),
        }
    }
    // Where the findings stand, by file and rule, so that a claim looks at
    // its own rule's findings of its own file only.
    let mut index: HashMap<PathBuf, HashMap<String, Vec<usize>>> = HashMap::new();
    for (n, d) in open.iter().enumerate() {
        index
            .entry(d.file.clone())
            .or_default()
            .entry(d.rule_id.clone())
            .or_default()
            .push(n);
    }
    let mut found: Vec<Option<Diagnostic>> = open.into_iter().map(Some).collect();
    let mut annotations = BTreeMap::new();
    for file in &project.files {
        let located = locate(project, &file.path);
        if located.is_empty() {
            continue;
        }
        let first_code = (gate.text)(&file.path)
            .map_or(u32::MAX, |text| first_code_line(project, &file.path, text));
        let mut reader = Reader {
            first_code,
            claims: Vec::new(),
            open: BTreeMap::new(),
            annotations: Vec::new(),
        };
        for at in &located {
            reader.read(at);
        }
        let Reader {
            claims,
            annotations: mut said,
            ..
        } = reader;
        let here = index.get(&file.path);
        for claim in &claims {
            let taken = take(&mut found, here, claim);
            said.push(claimed(gate, claim, !taken.is_empty()));
            let reason = claim.at.directive.reason.clone().unwrap_or_default();
            allowed.extend(taken.into_iter().map(|diagnostic| Suppressed {
                diagnostic,
                suppression: Suppression::in_source(reason.clone()),
            }));
        }
        annotations.insert(file.path.clone(), said);
    }
    Applied {
        kept: found.into_iter().flatten().collect(),
        allowed,
        annotations,
    }
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

/// What a claim of a directive says, for the rules that judge directives:
/// whether it suppressed a finding (`used`), and whether its rule could have
/// fired (`settled`), so that a claim that suppressed nothing is stale.
fn claimed(gate: &Gate, claim: &Claim, used: bool) -> Value {
    let mut value = annotation(claim.at, &claim.rule, None);
    value["used"] = json!(used);
    value["settled"] = json!(unused(gate, &claim.rule));
    value["range"] = json!(matches!(claim.coverage, Coverage::Range { .. }));
    value
}

/// Removes and returns the findings of the claim's rule that it reaches;
/// `here` is where the findings of the claim's file stand, by rule.
fn take(
    found: &mut [Option<Diagnostic>],
    here: Option<&HashMap<String, Vec<usize>>>,
    claim: &Claim,
) -> Vec<Diagnostic> {
    let slots = here
        .and_then(|rules| rules.get(&claim.rule))
        .map_or(&[][..], Vec::as_slice);
    let mut taken = Vec::new();
    for &n in slots {
        if found[n].as_ref().is_some_and(|d| reaches(claim, d)) {
            taken.extend(found[n].take());
        }
    }
    taken
}

fn reaches(claim: &Claim, d: &Diagnostic) -> bool {
    let line = d.span.start.line;
    match claim.coverage {
        Coverage::NextLine(at) => line == at || line == at + 1 || documents(claim, d),
        Coverage::Line(at) => line == at,
        Coverage::Range { from, to } => line >= from && to.is_none_or(|to| line < to),
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

/// What one rule named by a directive, or a directive that names none (`rule`
/// is empty), says: the rule and all the directive's rules, the reason (null
/// when it gives none), the marker and form as written, and where it is. A
/// claim is `used` when it suppressed something, `settled` when its rule could
/// have fired, `range` when it covers a range; `closes` is for `enable`, true
/// when it ended a range. `snippet` is what identifies it from one run to the
/// next, `place` where a finding about it is reported and `extent` the text a
/// fix removes.
fn annotation(at: &Located, rule: &str, closes: Option<bool>) -> Value {
    let comment = at.comment;
    let mut snippet = squash(&comment.text);
    if at.index > 0 {
        snippet.push_str(&format!(" #{}", at.index));
    }
    json!({
        "rule": rule,
        "rules": at.directive.rules,
        "reason": at.directive.reason,
        "marker": at.directive.marker,
        "form": at.directive.form.as_str(),
        "used": false,
        "settled": true,
        "range": false,
        "closes": closes.unwrap_or(true),
        "line": at.line,
        "snippet": snippet,
        "place": place(at),
        "extent": annotation_span(at),
    })
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
    u32::try_from(n).unwrap_or(u32::MAX)
}
