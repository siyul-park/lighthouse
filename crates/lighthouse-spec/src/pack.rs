use lighthouse_resource::Spec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Decision, Variable};

/// The spec of the `Pack` kind: an ordered set of sections, each an ordered
/// list of decisions. A decision belongs to the pack its id names (its
/// `<pack>/` prefix) and to the section of this list that names it.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackSpec {
    pub title: String,
    pub intro: String,
    /// Named CEL values the `cel` checks and fixes of the pack's decisions share.
    /// A decision uses one by naming it, as it uses one of its own `variables`:
    /// the pack's variables a decision names (directly, or through another) are
    /// put before its own, in this order, and see the same `options`, selected
    /// value and `file`. A variable that names none of those is evaluated once
    /// per file, however many subjects the file has. The name of a pack variable
    /// is not one of the decision's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<Variable>,
    pub sections: Vec<SectionSpec>,
}

impl Spec for PackSpec {
    const KIND: &'static str = "Pack";
}

/// A section of a pack, listing its decisions by name inside the pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionSpec {
    /// Kebab-case name of the section inside the pack.
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
    /// The CEL values the decisions of the pack share; see [`PackSpec::variables`].
    pub variables: Vec<Variable>,
    pub sections: Vec<Section>,
}
