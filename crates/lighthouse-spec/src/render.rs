use std::{collections::BTreeMap, fmt::Write};

use serde_json::Value;

use crate::{Catalog, Example, Fix, FixKind, OpSpec, OptionSpec, Pattern, pack_docs};

/// Markdown for every pack, keyed by path relative to the docs root.
pub fn docs(catalog: &Catalog) -> BTreeMap<String, String> {
    catalog
        .packs
        .iter()
        .map(|pack| {
            (
                format!("patterns/{}.md", pack.id),
                pack_docs::pack_markdown(pack),
            )
        })
        .collect()
}

/// One pattern as Markdown with its title at heading `level`.
pub fn pattern_markdown(pattern: &Pattern, level: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} {}\n", "#".repeat(level), pattern.title);
    let severity = pattern
        .severity()
        .map_or_else(|| "none".to_owned(), |s| s.to_string());
    let preset = if pattern.strict {
        " · preset `strict`"
    } else {
        ""
    };
    let _ = writeln!(
        out,
        "`{}` · scope `{}` · enforcement `{}` · severity `{severity}`{preset}\n",
        pattern.id, pattern.scope, pattern.enforcement
    );
    block(&mut out, "Intent", &pattern.intent);
    block(&mut out, "Requirement", &pattern.requirement);
    if let Some(exceptions) = &pattern.exceptions {
        block(&mut out, "Exceptions", exceptions);
    }
    if !pattern.options.is_empty() {
        options_table(&mut out, &pattern.options);
    }
    if let Some(fix) = &pattern.fix {
        fix_markdown(&mut out, fix);
    }
    for example in &pattern.examples {
        example_markdown(&mut out, example);
    }
    for (language, text) in &pattern.tuning {
        block(&mut out, &format!("Tuning: {language}"), text);
    }
    if let Some(citation) = &pattern.citation {
        block(&mut out, "Method", citation);
    }
    out
}

/// How a pattern's findings are fixed, for the pattern page.
fn fix_markdown(out: &mut String, fix: &Fix) {
    let how = match &fix.kind {
        FixKind::Ops(ops) => {
            let names: Vec<&str> = ops.iter().map(OpSpec::name).collect();
            format!("operations {}", names.join(", "))
        }
        FixKind::Command(command) => format!("command `{}`", command.argv.join(" ")),
        FixKind::Rpc(_) => "rpc".to_owned(),
    };
    let _ = writeln!(
        out,
        "**Fix**\n\nFixable: `{}` · {how}. See [fix operations](fix-operations.md).\n",
        fix.safety
    );
}

fn block(out: &mut String, label: &str, text: &str) {
    let _ = writeln!(out, "**{label}**\n\n{}\n", text.trim());
}

fn options_table(out: &mut String, options: &BTreeMap<String, OptionSpec>) {
    let _ = writeln!(out, "**Options**\n");
    let _ = writeln!(out, "| Option | Type | Default | Description |");
    let _ = writeln!(out, "| --- | --- | --- | --- |");
    for (name, spec) in options {
        let mut default = format!("`{}`", spec.default);
        for (language, value) in &spec.per_language {
            let _ = write!(default, "; {language}: `{value}`");
        }
        let _ = writeln!(
            out,
            "| `{name}` | {} | {default} | {} |",
            spec.kind,
            spec.description.trim()
        );
    }
    out.push('\n');
}

fn example_markdown(out: &mut String, example: &Example) {
    let kind = example.kind.to_string();
    let _ = writeln!(
        out,
        "**{} example: {} ({})**\n",
        capitalize(&kind),
        example.name,
        example.language
    );
    if !example.options.is_empty() {
        let options: Vec<_> = example
            .options
            .iter()
            .map(|(k, v)| format!("`{k}` = `{}`", plain(v)))
            .collect();
        let _ = writeln!(out, "Options: {}\n", options.join(", "));
    }
    for file in &example.files {
        if example.files.len() > 1 {
            let _ = writeln!(out, "`{}`\n", file.path);
        }
        let _ = writeln!(
            out,
            "```{}\n{}\n```\n",
            example.language,
            file.text().trim_end()
        );
    }
    for file in &example.fixed {
        let _ = writeln!(
            out,
            "Fixed `{}`:\n\n```{}\n{}\n```\n",
            file.path,
            example.language,
            file.text().trim_end()
        );
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
