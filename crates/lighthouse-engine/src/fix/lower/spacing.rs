//! The blank lines between declarations: the separation a scope uses between
//! its declarations, and the text a move inserts in that style, so that a
//! declaration put elsewhere is set apart as its neighbours are.

use std::{collections::BTreeMap, ops::Range};

use super::super::edits::{blank_count, blank_end, blank_start, indent_of};

/// The number of blank lines the declarations at `spans` (in file order) are
/// set apart by: the most common run between two that follow each other, the
/// larger when two are as common. Else the most common run after a
/// declaration that code follows at its own indentation, or before one that
/// code precedes at its own indentation. A scope with no evidence uses one.
pub(super) fn separation(text: &str, spans: &[Range<usize>]) -> usize {
    let runs: Vec<usize> = spans
        .windows(2)
        .filter(|w| w[0].end <= w[1].start && text[w[0].end..w[1].start].trim().is_empty())
        .map(|w| text[w[0].end..w[1].start].matches('\n').count())
        .collect();
    let runs = if runs.is_empty() {
        spans.iter().flat_map(|s| runs_around(text, s)).collect()
    } else {
        runs
    };
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
    for run in runs {
        *counts.entry(run).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|&(run, count)| (count, run))
        .map_or(1, |(run, _)| run)
}

/// The blank lines after `span` when code follows them, and before it when
/// code precedes them, each at the indentation of the declaration.
fn runs_around(text: &str, span: &Range<usize>) -> Vec<usize> {
    let indent = text[span.clone()]
        .lines()
        .find(|l| !l.trim().is_empty())
        .map_or(0, indent_of);
    let beside = |code: Option<&str>| code.is_some_and(|line| indent_of(line) >= indent);
    let mut runs = Vec::new();
    let below = span.end..blank_end(text, span.end);
    let next = text[below.end..].lines().next();
    if beside(next) {
        runs.push(blank_count(text, &below));
    }
    let above = blank_start(text, span.start)..span.start;
    let prev = text[..above.start].trim_end().lines().next_back();
    if beside(prev) {
        runs.push(blank_count(text, &above));
    }
    runs
}

/// What goes in after the line that ends at `at`: each of `bodies` (whole
/// lines) set apart by `separation` blank lines. When the text has no line end
/// there, it is made one and what is put in leaves the text as it was, without.
pub(super) fn after(
    text: &str,
    at: usize,
    separation: usize,
    bodies: &[String],
    eol: &str,
) -> String {
    let unterminated = !text[..at].ends_with('\n');
    let mut put = String::new();
    if unterminated {
        put.push_str(eol);
    }
    for body in bodies {
        put.push_str(&eol.repeat(separation));
        put.push_str(body);
    }
    if unterminated && let Some(kept) = put.strip_suffix(eol) {
        put.truncate(kept.len());
    }
    put
}

/// What goes in before a declaration: each of `bodies` (whole lines), then
/// `separation` blank lines.
pub(super) fn before(separation: usize, bodies: &[String], eol: &str) -> String {
    bodies
        .iter()
        .map(|body| format!("{body}{}", eol.repeat(separation)))
        .collect()
}
