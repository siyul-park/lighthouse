//! The engine's queries on the graph, over the file kinds of
//! `lighthouse-spec`: the units of a language and what a provider gave for
//! each, and the pull of what a run needs of the files.

mod detect;
mod open;
mod pull;
pub(crate) mod units;

pub(crate) use detect::Detected;
#[cfg(test)]
pub(crate) use units::set_units;
pub(crate) use units::{
    API, CLAIMED, EVERY, FRAGMENT, PROVIDER, Provider, Read, Stored, UNIT, UNITS,
};

#[cfg(test)]
mod tests;
