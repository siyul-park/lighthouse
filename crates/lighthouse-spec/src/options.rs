//! The options of a decision, declared as a JSON Schema object: one property
//! per option with its `type`, `default` and `description`. Rules read their
//! defaults from here, not from code.

use std::{collections::BTreeMap, fmt};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The JSON type the values of an option must have; `number` accepts any
/// number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OptionType {
    Integer,
    Number,
    Boolean,
    String,
    Array,
}

impl OptionType {
    pub(crate) fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::String => value.is_string(),
            Self::Array => value.is_array(),
        }
    }
}

impl OptionType {
    /// The JSON type name, as the schema spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::String => "string",
            Self::Array => "array",
        }
    }
}

impl fmt::Display for OptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Integer => "an integer",
            Self::Number => "a number",
            Self::Boolean => "a boolean",
            Self::String => "a string",
            Self::Array => "a list",
        })
    }
}

/// The only `type` of the options object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ObjectType {
    #[default]
    Object,
}

/// One tunable value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionSchema {
    #[serde(rename = "type")]
    pub kind: OptionType,
    pub default: Value,
    pub description: String,
}

/// A JSON Schema object that declares the options of a decision. Keys not
/// declared are refused: `additionalProperties` is `false`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionsSchema {
    #[serde(rename = "type", default)]
    pub kind: ObjectType,
    #[serde(default)]
    pub properties: BTreeMap<String, OptionSchema>,
    #[serde(default)]
    pub additional_properties: bool,
}

impl OptionsSchema {
    /// A schema over `properties` that refuses any other key.
    pub fn new(properties: BTreeMap<String, OptionSchema>) -> Self {
        Self {
            kind: ObjectType::Object,
            properties,
            additional_properties: false,
        }
    }
}
