use std::{collections::BTreeMap, fmt::Write};

use serde_json::Value;

use crate::{Catalog, Decision, Example, Fix, FixKind, OpSpec, OptionsSchema, pack_docs};

/// Where the generated decision pages live, relative to the docs root.
pub const DOCS_DIR: &str = "decisions";

/// Markdown for every pack, keyed by path relative to the docs root.
pub fn docs(catalog: &Catalog) -> BTreeMap<String, String> {
    catalog
        .packs
        .iter()
        .map(|pack| {
            (
                format!("{DOCS_DIR}/{}.md", pack.id),
                pack_docs::pack_markdown(pack),
            )
        })
        .collect()
}

/// Where a decision is explained in the generated docs: the pack page and,
/// for a decision that has an entry of its own, the heading anchor. The path
/// is relative to the repository root.
pub fn help_path(decision: &Decision) -> String {
    let page = format!("docs/{DOCS_DIR}/{}.md", decision.pack());
    if pack_docs::is_row(decision) {
        page
    } else {
        format!("{page}#{}", pack_docs::anchor(&decision.title))
    }
}

/// One decision as Markdown with its title at heading `level`.
pub fn decision_markdown(decision: &Decision, level: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} {}\n", "#".repeat(level), decision.title);
    let severity = decision
        .severity()
        .map_or_else(|| "none".to_owned(), |s| s.to_string());
    let provider = decision
        .check
        .as_ref()
        .map_or("none", |check| check.kind.label());
    let preset = if decision.strict() {
        " · preset `strict`"
    } else {
        ""
    };
    let mut lifecycle = String::new();
    if !decision.status.is_default() {
        let _ = write!(lifecycle, " · status `{}`", decision.status);
    }
    if !decision.supersedes.is_empty() {
        let _ = write!(
            lifecycle,
            " · supersedes {}",
            decision.supersedes.join(", ")
        );
    }
    let _ = writeln!(
        out,
        "`{}` · scope `{}` · severity `{severity}` · check `{provider}`{lifecycle}{preset}\n",
        decision.id(),
        decision.scope.subject,
    );
    if let Some(consequences) = &decision.consequences {
        block(&mut out, "Consequences", consequences);
    }
    block(&mut out, "Context", &decision.context);
    block(&mut out, "Requirement", &decision.requirement);
    if let Some(options) = decision
        .options
        .as_ref()
        .filter(|o| !o.properties.is_empty())
    {
        options_table(&mut out, decision, options);
    }
    if let Some(fix) = &decision.fix {
        fix_markdown(&mut out, fix);
    }
    for example in &decision.examples {
        example_markdown(&mut out, example);
    }
    if !decision.provenance.is_empty() {
        block(
            &mut out,
            "Derived from",
            &decision.provenance.was_derived_from.join("; "),
        );
    }
    out
}

/// How a decision's findings are fixed, for the decision page.
fn fix_markdown(out: &mut String, fix: &Fix) {
    let how = match &fix.kind {
        FixKind::Ops { ops } => {
            let names: Vec<&str> = ops.iter().map(OpSpec::name).collect();
            format!("operations {}", names.join(", "))
        }
        FixKind::Command(command) => format!("command `{}`", command.argv.join(" ")),
        FixKind::Rpc { .. } => "rpc".to_owned(),
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

fn options_table(out: &mut String, decision: &Decision, options: &OptionsSchema) {
    let _ = writeln!(out, "**Options**\n");
    let _ = writeln!(out, "| Option | Type | Default | Description |");
    let _ = writeln!(out, "| --- | --- | --- | --- |");
    for (name, property) in &options.properties {
        let mut default = format!("`{}`", property.default);
        for (language, spec) in &decision.languages {
            if let Some(value) = spec.options.get(name) {
                let _ = write!(default, "; {language}: `{value}`");
            }
        }
        let _ = writeln!(
            out,
            "| `{name}` | {} | {default} | {} |",
            property.describe(),
            property.description.trim()
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
