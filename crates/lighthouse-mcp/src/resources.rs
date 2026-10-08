//! Resources: the catalog index, one rendered decision, the effective config.

use std::{fmt::Write, fs};

use lighthouse_engine::active_rules;
use lighthouse_session::Session;
use lighthouse_spec::decision_markdown;
use rmcp::model::{Resource, ResourceTemplate};
use serde_json::json;

pub const CATALOG: &str = "lighthouse://catalog";
pub const CONFIG: &str = "lighthouse://config";
const DECISION_PREFIX: &str = "lighthouse://decisions/";
const MARKDOWN: &str = "text/markdown";

pub fn list() -> Vec<Resource> {
    vec![
        Resource::new(CATALOG, "catalog")
            .with_description("Index of the decision catalog: id, authored severity, status (only `accepted` decisions are enforced), title.")
            .with_mime_type("text/plain"),
        Resource::new(CONFIG, "config")
            .with_description("The effective configuration of this project.")
            .with_mime_type("application/json"),
    ]
}

pub fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new("lighthouse://decisions/{id}", "decision")
            .with_description(
                "One decision rendered as Markdown: intent, requirement, examples, exceptions. The id contains a slash, e.g. lighthouse://decisions/design/exported-doc.",
            )
            .with_mime_type(MARKDOWN),
    ]
}

/// The text and MIME type of the resource at `uri`; `Err` names what is
/// wrong with the uri.
pub fn read(uri: &str) -> Result<(String, &'static str), String> {
    let session = Session::load_or_default(None).map_err(|e| e.to_string())?;
    if uri == CATALOG {
        return catalog(&session).map(|text| (text, "text/plain"));
    }
    if uri == CONFIG {
        return config(&session).map(|text| (text, "application/json"));
    }
    let Some(id) = uri.strip_prefix(DECISION_PREFIX) else {
        return Err(format!("unknown resource `{uri}`"));
    };
    let catalog = session.catalog().map_err(|e| e.to_string())?;
    let decision = catalog
        .decision(id)
        .ok_or_else(|| format!("unknown decision `{id}`"))?;
    Ok((decision_markdown(decision, 1), MARKDOWN))
}

fn catalog(session: &Session) -> Result<String, String> {
    let catalog = session.catalog().map_err(|e| e.to_string())?;
    let mut out = String::new();
    for decision in catalog.decisions() {
        let tier = decision.severity().map_or_else(
            || "doc".to_owned(),
            |s| lighthouse_spec::authored_severity(s, Some(decision)).to_string(),
        );
        let _ = writeln!(
            out,
            "{}\t{tier}\t{}\t{}",
            decision.id(),
            decision.status,
            decision.title
        );
    }
    Ok(out)
}

fn config(session: &Session) -> Result<String, String> {
    let registry = session.in_process_registry().map_err(|e| e.to_string())?;
    let active = active_rules(&registry, &session.config).map_err(|e| e.to_string())?;
    let plugins: Vec<&str> = session
        .config
        .plugins()
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    let file = lighthouse_config::Config::file_in(&session.root)
        .and_then(|path| fs::read_to_string(path).ok());
    Ok(serde_json::to_string_pretty(&json!({
        "root": session.root.display().to_string(),
        "plugins": plugins,
        "extends": session.config.extends(),
        "activeRules": active,
        "file": file,
    }))
    .unwrap_or_default())
}
