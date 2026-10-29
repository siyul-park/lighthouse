//! The options of a decision, declared as a JSON Schema object: one property
//! per option with its `type`, `default` and `description`. Rules read their
//! defaults from here, not from code.

use std::{collections::BTreeMap, fmt, ops::Deref};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod shape;

/// The option of a decision that reports several kinds: a list whose items
/// are one of the declared kinds, of which the default switches some on.
pub(crate) const KINDS: &str = "kinds";

pub use shape::{LIMIT_ROLES, Shape, limit, merge};

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

/// One tunable value: its shape, default and description. The shape's keys
/// sit beside `default` and `description`; a key that is none of them is
/// refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "Map<String, Value>")]
pub struct OptionSchema {
    #[serde(flatten)]
    pub shape: Shape,
    pub default: Value,
    pub description: String,
    #[serde(default, rename = "strictDefault")]
    pub strict_default: bool,
}

impl OptionSchema {
    /// An option of one type.
    pub fn new(kind: OptionType, default: Value, description: impl Into<String>) -> Self {
        Self {
            shape: Shape::of(kind),
            default,
            description: description.into(),
            strict_default: false,
        }
    }
}

impl TryFrom<Map<String, Value>> for OptionSchema {
    type Error = String;

    fn try_from(mut keys: Map<String, Value>) -> Result<Self, String> {
        let mut take = |name: &str| keys.remove(name).ok_or(format!("missing field `{name}`"));
        let default = take("default")?;
        let description = take("description")?
            .as_str()
            .map(str::to_owned)
            .ok_or("`description` must be a string")?;
        let strict_default = match keys.remove("strictDefault") {
            None => false,
            Some(Value::Bool(value)) => value,
            Some(_) => return Err("`strictDefault` must be a boolean".to_owned()),
        };
        let shape = Shape::deserialize(Value::Object(keys)).map_err(|e| e.to_string())?;
        Ok(Self {
            shape,
            default,
            description,
            strict_default,
        })
    }
}

impl Deref for OptionSchema {
    type Target = Shape;

    fn deref(&self) -> &Shape {
        &self.shape
    }
}

/// A JSON Schema object that declares the options of a decision. Keys not
/// declared are refused: `additionalProperties` is `false`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionsSchema {
    #[serde(default)]
    pub properties: BTreeMap<String, OptionSchema>,
    #[serde(default)]
    pub additional_properties: bool,
    /// The decision whose options this one reads: the options it does not
    /// declare are those of that decision, as the project configured them. A
    /// project sets them there, once, and both decisions judge by them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The options read from `from`, once the decision is built over it.
    #[serde(skip)]
    #[schemars(skip)]
    pub inherited: Vec<String>,
    /// The values the languages of that decision give those options, by
    /// language: its own `languages` entries stay as authored, since they say
    /// where it applies.
    #[serde(skip)]
    #[schemars(skip)]
    pub inherited_languages: BTreeMap<String, Map<String, Value>>,
}

impl OptionsSchema {
    /// A schema over `properties` that refuses any other key.
    pub fn new(properties: BTreeMap<String, OptionSchema>) -> Self {
        Self {
            properties,
            additional_properties: false,
            from: None,
            inherited: Vec::new(),
            inherited_languages: BTreeMap::new(),
        }
    }
}
