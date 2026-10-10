use std::path::Path;

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{API_VERSION, Error, Resource, Spec};

/// A syntax a document can be written in. Every format reads into the same
/// JSON data model, so one set of types serves all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Toml,
    Json,
}

impl Format {
    /// The format a file name's extension says, if any.
    pub fn of_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "yaml" | "yml" => Some(Self::Yaml),
            "toml" => Some(Self::Toml),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// The documents of `text` as JSON values: any number for YAML (separated by
/// `---`), one for TOML, one or a list for JSON. Empty YAML documents are
/// skipped. `path` only names the text in errors.
pub fn documents(format: Format, path: &str, text: &str) -> Result<Vec<Value>, Error> {
    match format {
        Format::Yaml => yaml_documents(path, text),
        Format::Toml => toml::from_str::<Value>(text)
            .map(|doc| vec![doc])
            .map_err(|e| Error::parse(path, e)),
        Format::Json => {
            match serde_json::from_str::<Value>(text).map_err(|e| Error::parse(path, e))? {
                Value::Array(docs) => Ok(docs),
                doc => Ok(vec![doc]),
            }
        }
    }
}

/// The `kind` a parsed document declares.
pub fn kind_of(document: &Value) -> Option<&str> {
    document.get("kind")?.as_str()
}

/// Reads `document` as the resource of spec `S`. A document without an
/// `apiVersion` is refused.
pub fn resource<S: Spec + DeserializeOwned>(
    path: &str,
    document: &Value,
) -> Result<Resource<S>, Error> {
    let Some(version) = document.get("apiVersion").and_then(Value::as_str) else {
        return Err(Error::Unversioned {
            path: path.to_owned(),
            api_version: API_VERSION.to_owned(),
        });
    };
    let found = kind_of(document).unwrap_or("(none)");
    if version != API_VERSION || found != S::KIND {
        return Err(Error::Mismatch {
            path: path.to_owned(),
            expected: format!("{API_VERSION} {}", S::KIND),
            found: format!("{version} {found}"),
        });
    }
    serde_json::from_value(document.clone()).map_err(|e| Error::Invalid {
        path: path.to_owned(),
        message: e.to_string(),
    })
}

fn yaml_documents(path: &str, text: &str) -> Result<Vec<Value>, Error> {
    let mut docs = Vec::new();
    for doc in serde_norway::Deserializer::from_str(text) {
        let value = Value::deserialize(doc).map_err(|e| Error::parse(path, e))?;
        if !value.is_null() {
            docs.push(value);
        }
    }
    Ok(docs)
}
