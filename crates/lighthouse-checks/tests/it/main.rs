//! The integration tests of the crate, in one binary: the declarative
//! operations over synthetic projects, the bundled packs, the metrics and the
//! bundled registry against real language providers.

mod analyzers;
mod annotations;
mod bundled_fix;
mod core_pack;
mod cycle;
mod design_architecture;
mod design_layout;
mod design_rules;
mod directives;
mod fixes;
mod go;
mod layout;
mod rules;
mod rust;
mod support;
mod testing_rules;
