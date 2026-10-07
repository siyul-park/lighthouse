use lighthouse_model::{Comment, Diagnostic, Fingerprint};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

const ID: &str = "design/section-banners";

#[derive(Deserialize)]
struct Options {
    min_run: usize,
    rule_chars: String,
    labels: Vec<String>,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

/// Comments made only of banner lines: a run of rule characters, optionally
/// around a label, or a label marker such as `MARK:`. A comment that mixes
/// banner lines with prose is documentation (a table, a code block) and passes.
fn check(meta: &RuleManifest, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, text)) = ctx.file else {
        return Ok(Vec::new());
    };
    if ctx.project.file(&file.path).is_none_or(|f| f.generated) {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for comment in ctx.project.comments_in(&file.path) {
        let Some(label) = banner(comment, &options).filter(|_| standalone(text, comment)) else {
            continue;
        };
        let mut d = Diagnostic::new(
            &meta.id,
            meta.severity,
            "comment labels a section instead of stating a fact; name the code or split it instead",
            &file.path,
            comment.span,
            Fingerprint::of(&meta.id, &file.path.to_string_lossy(), &comment.text),
        );
        d.evidence = json!({ "comment": comment.text, "label": label });
        found.push(d);
    }
    Ok(found)
}

/// The label of a banner comment (empty for a bare rule line).
fn banner(comment: &Comment, options: &Options) -> Option<String> {
    let mut label = String::new();
    let mut lines = 0;
    for line in comment.text.lines() {
        let body = strip_markers(line)?;
        if body.is_empty() {
            continue;
        }
        lines += 1;
        let text = banner_line(body, options)?;
        if label.is_empty() {
            label = text;
        }
    }
    (lines > 0).then_some(label)
}

/// The text of a comment line without its markers; `None` for a doc comment
/// line, whose rule lines are Markdown.
fn strip_markers(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with("///") && !line.starts_with("////") || line.starts_with("//!") {
        return None;
    }
    let body = line
        .trim_start_matches("//")
        .trim_start_matches("/*")
        .trim_end_matches("*/")
        .trim_start_matches('#');
    Some(body.trim())
}

/// `Some(label)` when the line is a banner: a leading run of rule characters
/// at least `min_run` long, then an optional label and an optional trailing
/// run; or a label marker.
fn banner_line(body: &str, options: &Options) -> Option<String> {
    if let Some(marker) = options.labels.iter().find(|m| body.starts_with(m.as_str())) {
        return Some(body[marker.len()..].trim().to_owned());
    }
    let is_rule = |c: char| options.rule_chars.contains(c);
    let run = body.chars().take_while(|&c| is_rule(c)).count();
    if run < options.min_run {
        return None;
    }
    let rest = &body[body.char_indices().nth(run).map_or(body.len(), |(i, _)| i)..];
    Some(rest.trim().trim_end_matches(is_rule).trim().to_owned())
}

/// Whether only whitespace precedes the comment on its first line; a comment
/// that trails code belongs to that code.
fn standalone(text: &str, comment: &Comment) -> bool {
    let line = text
        .lines()
        .nth(comment.span.start.line.saturating_sub(1) as usize)
        .unwrap_or("");
    let before = line.get(..comment.span.start.col.saturating_sub(1) as usize);
    before.is_none_or(|b| b.trim().is_empty())
}
