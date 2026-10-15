//! The `core/formatted` check: a file equals what the project's formatter
//! makes of it. One finding per file, at the first line that differs. A
//! formatter that fails leaves the analysis incomplete, never clean; one that
//! may not run, because the project is not trusted, is skipped with a notice.

use lighthouse_model::{Diagnostic, Position, Span};
use lighthouse_plugin::{Ctx, Error as PluginError, RuleManifest};
use serde_json::json;

/// Checks the focused file against its language's formatter; a file of a
/// language with no formatter is not judged.
pub(crate) fn check(meta: &RuleManifest, ctx: &Ctx) -> Result<Vec<Diagnostic>, PluginError> {
    let Some((file, text)) = ctx.file else {
        return Ok(Vec::new());
    };
    let Some(formatter) = ctx.ws.formatters.get(&file.lang) else {
        return Ok(Vec::new());
    };
    let name = &formatter.argv[0];
    if !ctx.trusted {
        ctx.notices.push(format!(
            "not checked for formatting: the formatter `{name}` runs only in a trusted project (`lighthouse trust`)"
        ));
        return Ok(Vec::new());
    }
    let formatted = formatter
        .format(&ctx.ws.root, &file.path, text)
        .map_err(PluginError::Incomplete)?;
    if formatted == text {
        return Ok(Vec::new());
    }
    let line = first_difference(text, &formatted);
    let path = file.path.to_string_lossy().replace('\\', "/");
    let at = Position { line, col: 1 };
    let mut found = Diagnostic::new(
        &meta.id,
        meta.severity,
        format!("{path} differs from {name} output from line {line}; run the formatter"),
        &file.path,
        Span { start: at, end: at },
        meta.fingerprint(&path, ""),
    );
    found.evidence = json!({ "formatter": name, "line": line });
    Ok(vec![found])
}

/// The 1-based line of `text` where it first differs from `formatted`: the
/// last line when one is a prefix of the other.
fn first_difference(text: &str, formatted: &str) -> u32 {
    let mut ours = text.split_inclusive('\n');
    let mut theirs = formatted.split_inclusive('\n');
    let mut line: u32 = 1;
    loop {
        match (ours.next(), theirs.next()) {
            (Some(a), Some(b)) if a == b => line += 1,
            (None, Some(_)) => return line.saturating_sub(1).max(1),
            _ => return line,
        }
    }
}
