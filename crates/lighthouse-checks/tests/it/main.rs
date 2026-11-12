//! The integration tests of the crate, in one binary: the declarative
//! operations over synthetic projects, the bundled packs, the metrics and the
//! bundled registry against real language providers.

mod analyzers;
mod annotations;
mod bundled_fix;
mod bundled_manifests;
mod command_sarif;
mod core_pack;
mod cycle;
mod design_architecture;
mod design_layout;
mod design_placement;
mod design_rules;
mod directives;
mod edge_anchor;
mod fixes;
mod func_facts;
mod go;
mod ir_subjects;
mod layout;
mod level_solve;
mod levels;
mod library;
mod line_widths;
mod model_selects;
mod module_types;
mod pool;
mod probe;
mod region_options;
mod rules;
mod rust;
mod split_facts;
mod stability;
mod step_facts;
mod support;
mod testing_rules;
mod unit_facts;
