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
    Null,
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
            Self::Null => value.is_null(),
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
            Self::Null => "null",
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
            Self::Null => "null",
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

/// The named shapes a `$ref` may point to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ShapeRef {
    /// A limit: a non-negative integer, or an object that gives one per role
    /// of a function (see [`Shape::limit`]).
    #[serde(rename = "#/$defs/limit")]
    Limit,
}

/// The roles a limit may name, besides `default`.
pub const LIMIT_ROLES: [&str; 6] = [
    "function",
    "method",
    "constructor",
    "implementation",
    "entrypoint",
    "test",
];

/// The shape of a value inside an option: its type and, for a list, the shape
/// of its items, for an object, its properties and which of them are required.
/// With `oneOf`, a value is accepted when exactly one of the shapes accepts it;
/// with `$ref`, the shape is the named one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<OptionType>,
    /// Alternatives, exactly one of which a value must match.
    #[serde(rename = "oneOf", default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<Shape>,
    /// A named shape defined once by the spec.
    #[serde(rename = "$ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<ShapeRef>,
    /// The least an integer may be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,
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

impl Shape {
    /// A shape of one type.
    pub fn of(kind: OptionType) -> Self {
        Self {
            kind: Some(kind),
            ..Self::default()
        }
    }

    fn natural() -> Self {
        Self {
            minimum: Some(0),
            ..Self::of(OptionType::Integer)
        }
    }

    /// A non-negative integer, or null: no limit.
    fn bound() -> Self {
        Self {
            one_of: vec![Self::natural(), Self::of(OptionType::Null)],
            ..Self::default()
        }
    }

    /// The `limit` shape every limit option shares: a non-negative integer for
    /// every function, or an object that gives `default` and one per role.
    /// Null is no limit; a role left out falls back to `default`, and a
    /// `default` left out is no limit.
    pub fn limit() -> Self {
        let roles = std::iter::once("default")
            .chain(LIMIT_ROLES)
            .map(|role| (role.to_owned(), Self::bound()));
        Self {
            one_of: vec![
                Self::natural(),
                Self {
                    properties: roles.collect(),
                    ..Self::of(OptionType::Object)
                },
            ],
            ..Self::default()
        }
    }

    fn resolved(&self) -> std::borrow::Cow<'_, Shape> {
        match self.reference {
            Some(ShapeRef::Limit) => std::borrow::Cow::Owned(Self::limit()),
            None => std::borrow::Cow::Borrowed(self),
        }
    }

    /// Whether `value` has this shape; otherwise what is wrong, naming `at`.
    pub(crate) fn check(&self, value: &Value, at: &str) -> Result<(), String> {
        let shape = self.resolved();
        if !shape.one_of.is_empty() {
            return shape.check_one_of(value, at);
        }
        let Some(kind) = shape.kind else {
            return Ok(());
        };
        if !kind.accepts(value) {
            return Err(format!("{at} must be {kind}"));
        }
        if let (Some(least), Some(n)) = (shape.minimum, value.as_i64())
            && n < least
        {
            return Err(format!("{at} must be at least {least}"));
        }
        match value {
            Value::Array(list) => {
                if let Some(items) = &shape.items {
                    for (n, item) in list.iter().enumerate() {
                        items.check(item, &format!("{at}[{n}]"))?;
                    }
                }
            }
            Value::Object(map) => shape.check_object(map, at)?,
            _ => {}
        }
        Ok(())
    }

    fn check_object(&self, map: &serde_json::Map<String, Value>, at: &str) -> Result<(), String> {
        for key in &self.required {
            if !map.contains_key(key) {
                return Err(format!("{at} needs `{key}`"));
            }
        }
        for (key, member) in map {
            match self.properties.get(key) {
                Some(shape) => shape.check(member, &format!("{at}.{key}"))?,
                None if !self.properties.is_empty() => {
                    return Err(format!("{at} has an unknown key `{key}`"));
                }
                None => {}
            }
        }
        Ok(())
    }

    fn check_one_of(&self, value: &Value, at: &str) -> Result<(), String> {
        let results: Vec<Result<(), String>> =
            self.one_of.iter().map(|s| s.check(value, at)).collect();
        match results.iter().filter(|r| r.is_ok()).count() {
            1 => Ok(()),
            0 => {
                let problems: Vec<&str> = results
                    .iter()
                    .filter_map(|r| r.as_ref().err().map(String::as_str))
                    .collect();
                Err(format!(
                    "{at} matches none of the allowed shapes: {}",
                    problems.join("; ")
                ))
            }
            _ => Err(format!("{at} matches more than one of the allowed shapes")),
        }
    }

    /// What a value of this shape is, for a message.
    pub(crate) fn describe(&self) -> String {
        let shape = self.resolved();
        if !shape.one_of.is_empty() {
            let parts: Vec<String> = shape.one_of.iter().map(Shape::describe).collect();
            return parts.join(" or ");
        }
        shape
            .kind
            .map_or_else(|| "any value".to_owned(), |k| k.to_string())
    }
}

/// One tunable value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionSchema {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<OptionType>,
    pub default: Value,
    pub description: String,
    /// Alternatives, exactly one of which a value must match.
    #[serde(rename = "oneOf", default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<Shape>,
    /// A named shape defined once by the spec, such as `#/$defs/limit`.
    #[serde(rename = "$ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<ShapeRef>,
    /// The least an integer may be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,
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
    /// An option of one type.
    pub fn new(kind: OptionType, default: Value, description: impl Into<String>) -> Self {
        Self {
            kind: Some(kind),
            default,
            description: description.into(),
            one_of: Vec::new(),
            reference: None,
            minimum: None,
            items: None,
            properties: BTreeMap::new(),
            required: Vec::new(),
        }
    }

    /// The shape a value of this option must have.
    pub fn shape(&self) -> Shape {
        Shape {
            kind: self.kind,
            one_of: self.one_of.clone(),
            reference: self.reference,
            minimum: self.minimum,
            items: self.items.clone(),
            properties: self.properties.clone(),
            required: self.required.clone(),
        }
    }

    /// Whether `value` has this shape; otherwise what is wrong, naming `at`.
    pub(crate) fn check(&self, value: &Value, at: &str) -> Result<(), String> {
        self.shape().check(value, at)
    }

    /// What a value of this option is, for a message.
    pub(crate) fn describe(&self) -> String {
        self.shape().describe()
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
