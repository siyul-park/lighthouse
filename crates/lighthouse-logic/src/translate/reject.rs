//! What leaves the fragment: the construct a rejection names, as a stable
//! token for counting and as prose for people.

#[cfg(test)]
mod tests;

use std::fmt;

/// A construct the translator does not read. `as_str` is its token (the
/// reason of a tier), `Display` its prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Construct {
    Unused,
    Compile(String),
    Expansion,
    Conjunction,
    Cases,
    Arity(String),
    Bare(String),
    PackVariable(String),
    ConstantField,
    ComputedList,
    ComputedComprehension,
    Comprehension,
    All,
    Has,
    Expression,
    NegatedField,
    NegatedExists,
    Concatenation,
    Arithmetic,
    ListCondition,
    ConditionList,
    Membership,
    Size,
    Count,
    TwoFields,
    Order,
    Overflow,
    Literal,
    Ternary,
    Index,
    In,
    /// A call, by name (`flatten`, `regex.matches`).
    Call(String),
}

impl Construct {
    /// The token: stable, one per construct (several prose share it).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Unused => "unused-bind",
            Self::Compile(_) => "compile",
            Self::Expansion | Self::Conjunction | Self::Cases => "expansion",
            Self::Arity(_) | Self::Bare(_) => "arguments",
            Self::PackVariable(_) => "pack-variable",
            Self::ConstantField => "constant-field",
            Self::ComputedList => "computed-list",
            Self::ComputedComprehension => "computed-comprehension",
            Self::Comprehension => "comprehension",
            Self::All => "all",
            Self::Has => "has",
            Self::Expression => "expression",
            Self::NegatedField => "negated-field",
            Self::NegatedExists => "negated-exists",
            Self::Concatenation => "concatenation",
            Self::Arithmetic => "arithmetic",
            Self::ListCondition => "list-condition",
            Self::ConditionList => "condition-list",
            Self::Membership => "membership",
            Self::Size => "size",
            Self::Count => "count",
            Self::TwoFields => "two-fields",
            Self::Order => "order",
            Self::Overflow => "overflow",
            Self::Literal => "literal",
            Self::Ternary => "ternary",
            Self::Index => "index",
            Self::In => "in",
            Self::Call(name) => name,
        }
    }
}

impl fmt::Display for Construct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unused => f.write_str("a bound value that is never read"),
            Self::Compile(why) => write!(f, "an expression that does not compile: {why}"),
            Self::Expansion => f.write_str("a condition that expands too far"),
            Self::Conjunction => f.write_str("a conjunction of too many conditions"),
            Self::Cases => f.write_str("a condition that expands to too many cases"),
            Self::Arity(name) => write!(f, "`{name}` called with other arguments"),
            Self::Bare(name) => write!(f, "`{name}` without arguments"),
            Self::PackVariable(name) => write!(f, "undefined name or pack variable `{name}`"),
            Self::ConstantField => f.write_str("a field of a constant"),
            Self::ComputedList => f.write_str("a list that is computed"),
            Self::ComputedComprehension => {
                f.write_str("a comprehension as a value or over a computed list")
            }
            Self::Comprehension => f.write_str("a comprehension"),
            Self::All => f.write_str("`all`"),
            Self::Has => f.write_str("`has()`"),
            Self::Expression => f.write_str("an expression that is not a value or a condition"),
            Self::NegatedField => f.write_str("negation of a field"),
            Self::NegatedExists => f.write_str("negation of an exists with a condition"),
            Self::Concatenation => f.write_str("concatenation or arithmetic"),
            Self::Arithmetic => f.write_str("arithmetic outside a comparison with a number"),
            Self::ListCondition => f.write_str("a list as a condition"),
            Self::ConditionList => f.write_str("a condition as a list"),
            Self::Membership => f.write_str("membership in a field"),
            Self::Size => f.write_str("a size that is not compared with a literal"),
            Self::Count => f.write_str("a count of members other than none or some"),
            Self::TwoFields => f.write_str("a comparison of two fields"),
            Self::Order => f.write_str("an order between values that are not numbers or texts"),
            Self::Overflow => f.write_str("an integer that overflows"),
            Self::Literal => f.write_str("a literal that is not a bool, int, string or null"),
            Self::Ternary => f.write_str("ternary"),
            Self::Index => f.write_str("index"),
            Self::In => f.write_str("in"),
            Self::Call(name) => write!(f, "the call `{name}`"),
        }
    }
}

/// The construct that leaves the fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject(pub Construct);

impl Reject {
    /// The construct's token.
    pub fn construct(&self) -> &str {
        self.0.as_str()
    }
}

impl From<Construct> for Reject {
    fn from(construct: Construct) -> Self {
        Self(construct)
    }
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outside the fragment: {}", self.0)
    }
}

impl std::error::Error for Reject {}
