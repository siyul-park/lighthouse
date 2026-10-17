//! Resources: the catalog index, one rendered decision, the effective config.

use lighthouse_session::{Session, catalog_index, config_summary, decision_text};
use rmcp::model::{Resource, ResourceTemplate};

pub const CATALOG: &str = "lighthouse://catalog";
pub const CONFIG: &str = "lighthouse://config";
const DECISIONS: &str = "lighthouse://decisions/";
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
        ResourceTemplate::new(format!("{DECISIONS}{{id}}"), "decision")
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
        return catalog_index(&session)
            .map(|text| (text, "text/plain"))
            .map_err(|e| e.to_string());
    }
    if uri == CONFIG {
        return config_summary(&session)
            .and_then(|summary| Ok(serde_json::to_string_pretty(&summary)?))
            .map(|text| (text, "application/json"))
            .map_err(|e| e.to_string());
    }
    let Some(id) = uri.strip_prefix(DECISIONS) else {
        return Err(format!("unknown resource `{uri}`"));
    };
    decision_text(&session, id)
        .map(|text| (text, MARKDOWN))
        .map_err(|e| e.to_string())
}
