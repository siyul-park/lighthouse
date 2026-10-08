use schemars::JsonSchema;
use serde_json::Value;

use crate::{Resource, Spec};

/// Where the checked-in schemas are published; `$id` of each is below it.
pub const SCHEMA_URL_BASE: &str =
    "https://raw.githubusercontent.com/siyul-park/lighthouse/main/schema";

/// One kind and its JSON Schema.
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub kind: &'static str,
    pub schema: Value,
}

impl Descriptor {
    /// The descriptor of the kind whose spec is `S`.
    pub fn of<S: Spec + JsonSchema>() -> Self {
        Self {
            kind: S::KIND,
            schema: schema::<S>(),
        }
    }
}

/// The JSON Schema (draft 2020-12) of a whole document of spec `S`.
pub fn schema<S: Spec + JsonSchema>() -> Value {
    let mut root =
        serde_json::to_value(schemars::schema_for!(Resource<S>)).expect("a schema serializes");
    if let Value::Object(map) = &mut root {
        map.insert(
            "$id".to_owned(),
            Value::String(format!("{SCHEMA_URL_BASE}/{}", schema_file(S::KIND))),
        );
        map.insert("title".to_owned(), Value::String(S::KIND.to_owned()));
    }
    root
}

/// The file name of a kind's schema: `DecisionOverride` is
/// `decision-override.schema.json`.
pub fn schema_file(kind: &str) -> String {
    let mut name = String::new();
    for (i, c) in kind.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                name.push('-');
            }
            name.push(c.to_ascii_lowercase());
        } else {
            name.push(c);
        }
    }
    format!("{name}.schema.json")
}

/// The comment that makes editors validate and complete a YAML document of
/// `kind`; `base` is the directory or URL holding the schemas.
pub fn header(kind: &str, base: &str) -> String {
    format!(
        "# yaml-language-server: $schema={}/{}\n",
        base.trim_end_matches('/'),
        schema_file(kind)
    )
}
