use std::collections::BTreeMap;

use lighthouse_model::{Options, Severity};
use serde::{Deserialize, Deserializer, de};

pub type Rules = BTreeMap<String, RuleConfig>;

/// `"warn"`, `"off"`, or `{ level = "warn", max = 10 }`.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleConfig {
    /// `None` disables the rule.
    pub level: Option<Severity>,
    pub options: Options,
}

impl<'de> Deserialize<'de> for RuleConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match toml::Value::deserialize(d)? {
            toml::Value::String(level) => Ok(Self {
                level: level_of(&level).map_err(de::Error::custom)?,
                options: Options::new(),
            }),
            toml::Value::Table(mut table) => {
                let level = match table.remove("level") {
                    Some(toml::Value::String(level)) => level,
                    _ => return Err(de::Error::custom("rule table requires a string `level`")),
                };
                let options = match serde_json::to_value(table).map_err(de::Error::custom)? {
                    serde_json::Value::Object(map) => map,
                    _ => unreachable!("a toml table serializes to an object"),
                };
                Ok(Self {
                    level: level_of(&level).map_err(de::Error::custom)?,
                    options,
                })
            }
            _ => Err(de::Error::custom(
                "rule must be a level string or a table with `level`",
            )),
        }
    }
}

fn level_of(s: &str) -> Result<Option<Severity>, String> {
    if s == "off" {
        return Ok(None);
    }
    s.parse().map(Some).map_err(|e| format!("{e} or off"))
}

/// Later level replaces the earlier one; options merge per key, later wins.
pub(crate) fn merge(into: &mut Rules, from: &Rules) {
    for (id, next) in from {
        match into.get_mut(id) {
            Some(prev) => {
                prev.level = next.level;
                prev.options.extend(next.options.clone());
            }
            None => {
                into.insert(id.clone(), next.clone());
            }
        }
    }
}
