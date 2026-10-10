//! The standard resource model of every Lighthouse spec document: an
//! envelope (`apiVersion`, `kind`, `metadata`, `spec`) over a kind-specific
//! spec, readable from YAML, TOML or JSON, writable as readable YAML, and
//! described by a JSON Schema per kind.

mod document;
mod duration;
mod error;
mod schema;
mod yaml;

use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use document::{Format, documents, kind_of, resource, yaml_values};
pub use duration::parse_duration;
pub use error::Error;
pub use schema::{Descriptor, SCHEMA_URL_BASE, header, schema, schema_file};
pub use yaml::to_yaml;

/// The first of `names` that is a file in `dir`: how a project's config and a
/// plugin's manifest are found, in the order the names are tried.
pub fn file_in(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// The version every document of this model declares.
pub const API_VERSION: &str = "lighthouse/v1alpha1";

/// The content of one kind of resource.
pub trait Spec {
    /// The `kind` of a document with this content, such as `Decision`.
    const KIND: &'static str;
}

/// Who a resource is. Identity is `name`; grouping is by label, never by
/// where the document is stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// Fully qualified name, such as `design/minimal-names`.
    pub name: String,
    /// Selectable facts, keyed `lighthouse/<name>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Free notes; never used to select.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

impl Metadata {
    /// Metadata with a name and nothing else.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// A document: the envelope around the spec `S`.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource<S> {
    pub metadata: Metadata,
    pub spec: S,
}

impl<S> Resource<S> {
    /// A resource with this identity and content.
    pub fn new(metadata: Metadata, spec: S) -> Self {
        Self { metadata, spec }
    }
}

impl<S: Spec + Serialize> Serialize for Resource<S> {
    fn serialize<T: Serializer>(&self, serializer: T) -> Result<T::Ok, T::Error> {
        Out {
            api_version: API_VERSION,
            kind: S::KIND,
            metadata: &self.metadata,
            spec: &self.spec,
        }
        .serialize(serializer)
    }
}

impl<'de, S: Spec + Deserialize<'de>> Deserialize<'de> for Resource<S> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let envelope = In::<S>::deserialize(deserializer)?;
        if envelope.api_version != API_VERSION {
            return Err(D::Error::custom(format!(
                "apiVersion is `{}`, expected `{API_VERSION}`",
                envelope.api_version
            )));
        }
        if envelope.kind != S::KIND {
            return Err(D::Error::custom(format!(
                "kind is `{}`, expected `{}`",
                envelope.kind,
                S::KIND
            )));
        }
        Ok(Self {
            metadata: envelope.metadata,
            spec: envelope.spec,
        })
    }
}

impl<S: Spec + JsonSchema> JsonSchema for Resource<S> {
    /// The kind: one schema per kind of document.
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed(S::KIND)
    }

    /// An object with the four fields of the envelope; `apiVersion` and
    /// `kind` are constants, `spec` is the schema of `S`.
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "properties": {
                "apiVersion": { "const": API_VERSION },
                "kind": { "const": S::KIND },
                "metadata": generator.subschema_for::<Metadata>(),
                "spec": generator.subschema_for::<S>(),
            },
            "required": ["apiVersion", "kind", "metadata", "spec"],
            "additionalProperties": false,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Out<'a, S> {
    api_version: &'static str,
    kind: &'static str,
    metadata: &'a Metadata,
    spec: &'a S,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct In<S> {
    api_version: String,
    kind: String,
    metadata: Metadata,
    spec: S,
}

/// A document in YAML, with the comment that makes editors validate it
/// against its schema; `schema_base` is the directory or URL holding them.
pub fn to_document<S: Spec + Serialize>(resource: &Resource<S>, schema_base: &str) -> String {
    format!("{}{}", header(S::KIND, schema_base), to_yaml(resource))
}
