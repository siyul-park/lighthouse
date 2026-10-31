//! The envelope of every spec document (`apiVersion`, `kind`, `metadata`,
//! `spec`) over a kind-specific spec, and the identity it carries.

use std::{borrow::Cow, collections::BTreeMap};

use schemars::{JsonSchema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::Value;

use crate::Error;

/// The version every document of this model declares.
pub const API_VERSION: &str = "lighthouse/v1alpha1";

/// The content of one kind of resource.
pub trait Spec {
    /// The `kind` of a document with this content, such as `Decision`.
    const KIND: &'static str;
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
    fn json_schema(generator: &mut SchemaGenerator) -> schemars::Schema {
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

/// Who a resource is. Identity is `name`; grouping is by label, never by
/// where the document is stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// Fully qualified name, such as `design/no-redundant-qualifier`.
    pub name: String,
    /// Identity that outlives the name: a UUID v4 assigned once, never derived
    /// from the name (Kubernetes `metadata.uid`). Renaming a resource keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
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

/// A fresh uid: a random UUID v4 in its canonical lowercase form.
pub fn new_uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Whether `text` is a uid: a UUID v4 in its canonical lowercase form.
pub fn is_uid(text: &str) -> bool {
    uuid::Uuid::parse_str(text)
        .is_ok_and(|u| u.get_version() == Some(uuid::Version::Random) && u.to_string() == text)
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
    let version = document.get("apiVersion").and_then(Value::as_str);
    let found = kind_of(document).unwrap_or("(none)");
    if version != Some(API_VERSION) || found != S::KIND {
        return Err(Error::Mismatch {
            path: path.to_owned(),
            expected: format!("{API_VERSION} {}", S::KIND),
            found: version.map_or_else(|| "(none)".to_owned(), |v| format!("{v} {found}")),
        });
    }
    serde_json::from_value(document.clone()).map_err(|e| Error::Invalid {
        path: path.to_owned(),
        message: e.to_string(),
    })
}
