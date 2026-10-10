//! The shape of a value inside an option, and the named shapes the spec
//! defines once. One registry of named shapes serves both the JSON Schema
//! (`$defs`) and the checking of a value (`$ref`).

use std::{collections::BTreeMap, sync::OnceLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::OptionType;

/// The roles a limit may name, besides `default`.
pub const LIMIT_ROLES: [&str; 6] = [
    "function",
    "method",
    "constructor",
    "implementation",
    "entrypoint",
    "test",
];

/// What a limit of `-1` says: there is none.
pub const NO_LIMIT: i64 = -1;

const DEFS: &str = "#/$defs/";

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
    /// A named shape defined once by the spec, such as `#/$defs/limit`.
    #[serde(rename = "$ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
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

    /// The shape the `$ref` names, if this is a reference to a known one.
    pub fn target(&self) -> Option<&'static Shape> {
        let name = self.reference.as_deref()?.strip_prefix(DEFS)?;
        definitions().get(name)
    }

    /// Whether `value` has this shape; otherwise what is wrong, naming `at`.
    pub fn check(&self, value: &Value, at: &str) -> Result<(), String> {
        let shape = self.target().unwrap_or(self);
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

    fn check_object(&self, map: &Map<String, Value>, at: &str) -> Result<(), String> {
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
    pub fn describe(&self) -> String {
        let shape = self.target().unwrap_or(self);
        if !shape.one_of.is_empty() {
            let parts: Vec<String> = shape.one_of.iter().map(Shape::describe).collect();
            return parts.join(" or ");
        }
        shape
            .kind
            .map_or_else(|| "any value".to_owned(), |k| k.to_string())
    }
}

/// The named shapes of the spec, by name.
pub fn definitions() -> &'static BTreeMap<String, Shape> {
    static DEFS_BY_NAME: OnceLock<BTreeMap<String, Shape>> = OnceLock::new();
    DEFS_BY_NAME.get_or_init(|| BTreeMap::from([("limit".to_owned(), limit())]))
}

/// `over` laid on `base`: two objects merge per key, anything else is `over`.
/// An object-valued option (a limit per role) is thus refined, not replaced,
/// by each layer that sets it.
pub fn merge(base: &Value, over: &Value) -> Value {
    let (Value::Object(base), Value::Object(over)) = (base, over) else {
        return over.clone();
    };
    let mut merged = base.clone();
    for (key, value) in over {
        merged.insert(key.clone(), value.clone());
    }
    Value::Object(merged)
}

/// The `limit` shape every limit option shares: an integer for every role of
/// a function, or an object that gives `default` and one per role. `-1` is no
/// limit; a role left out takes `default`, and a `default` left out is no
/// limit.
fn limit() -> Shape {
    let bound = Shape {
        minimum: Some(NO_LIMIT),
        ..Shape::of(OptionType::Integer)
    };
    let roles = std::iter::once("default")
        .chain(LIMIT_ROLES)
        .map(|role| (role.to_owned(), bound.clone()));
    Shape {
        one_of: vec![
            bound.clone(),
            Shape {
                properties: roles.collect(),
                ..Shape::of(OptionType::Object)
            },
        ],
        ..Shape::default()
    }
}
