use lighthouse_resource::Spec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Decision;

/// The spec of the `Pack` kind: an ordered set of sections, each an ordered
/// list of decisions. A decision belongs to a pack and section by its labels
/// (`lighthouse/pack`, `lighthouse/section`); this list only orders them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackSpec {
    pub title: String,
    pub intro: String,
    pub sections: Vec<SectionSpec>,
}

impl Spec for PackSpec {
    const KIND: &'static str = "Pack";
}

/// A section of a pack, listing its decisions by name inside the pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionSpec {
    /// Kebab-case name; the value of the decisions' section label.
    pub name: String,
    pub title: String,
    pub intro: String,
    /// Names of the decisions, without the pack prefix, in reading order.
    pub decisions: Vec<String>,
}

/// An ordered group of decisions within a pack.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub intro: String,
    pub decisions: Vec<Decision>,
}

/// An ordered set of sections; the pack id is the prefix of its decision ids.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Pack {
    pub id: String,
    pub title: String,
    pub intro: String,
    pub sections: Vec<Section>,
}
