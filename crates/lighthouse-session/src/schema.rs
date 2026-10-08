//! The JSON Schema of every kind of spec document and log record, gathered
//! from the crates that define them.

use std::collections::BTreeMap;

use lighthouse_resource::{Descriptor, schema_file};

/// Every kind with its schema, by kind name.
pub fn schemas() -> BTreeMap<&'static str, Descriptor> {
    [
        lighthouse_spec::descriptors(),
        lighthouse_config::descriptors(),
        lighthouse_plugin::descriptors(),
        lighthouse_rpc::descriptors(),
        lighthouse_store::descriptors(),
    ]
    .into_iter()
    .flatten()
    .map(|d| (d.kind, d))
    .collect()
}

/// The schema of the kind named `kind`, in any letter case.
pub fn schema_of(kind: &str) -> Option<Descriptor> {
    schemas()
        .into_values()
        .find(|d| d.kind.eq_ignore_ascii_case(kind) || schema_file(d.kind) == kind)
}
