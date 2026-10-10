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
    Object,
}

impl OptionType {
    pub(crate) fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::String => value.is_string(),
            Self::Array => value.is_array(),
            Self::Object => value.is_object(),
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
            Self::Object => "object",
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
            Self::Object => "an object",
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

/// The shape of a value inside an option: its type and, for a list, the shape
/// of its items, for an object, its properties and which of them are required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    #[serde(rename = "type")]
    pub kind: OptionType,
    /// What each item of a list is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Shape>>,
    /// The keys of an object; an object with properties refuses any other key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, Shape>,
    /// The keys an object must have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

/// One tunable value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionSchema {
    #[serde(rename = "type")]
    pub kind: OptionType,
    pub default: Value,
    pub description: String,
    /// What each item of a list is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Shape>>,
    /// The keys of an object; an object with properties refuses any other key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, Shape>,
    /// The keys an object must have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

impl OptionSchema {
    /// Whether `value` has this shape; otherwise what is wrong, naming `at`.
    pub(crate) fn check(&self, value: &Value, at: &str) -> Result<(), String> {
        check(
            self.kind,
            self.items.as_deref(),
            &self.properties,
            &self.required,
            value,
            at,
        )
    }
}

impl Shape {
    fn check(&self, value: &Value, at: &str) -> Result<(), String> {
        check(
            self.kind,
            self.items.as_deref(),
            &self.properties,
            &self.required,
            value,
            at,
        )
    }
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

fn check(
    kind: OptionType,
    items: Option<&Shape>,
    properties: &BTreeMap<String, Shape>,
    required: &[String],
    value: &Value,
    at: &str,
) -> Result<(), String> {
    if !kind.accepts(value) {
        return Err(format!("{at} must be {kind}"));
    }
    match value {
        Value::Array(list) => {
            for (n, item) in list.iter().enumerate() {
                if let Some(shape) = items {
                    shape.check(item, &format!("{at}[{n}]"))?;
                }
            }
        }
        Value::Object(map) => {
            for key in required {
                if !map.contains_key(key) {
                    return Err(format!("{at} needs `{key}`"));
                }
            }
            for (key, member) in map {
                match properties.get(key) {
                    Some(shape) => shape.check(member, &format!("{at}.{key}"))?,
                    None if !properties.is_empty() => {
                        return Err(format!("{at} has an unknown key `{key}`"));
                    }
                    None => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}
