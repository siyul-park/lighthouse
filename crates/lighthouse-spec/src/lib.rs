mod catalog;
mod check;
mod decision;
mod error;
mod fix;
mod fix_docs;
mod legacy;
mod load;
mod migrate;
mod model;
mod options;
mod overrides;
mod pack;
mod pack_docs;
mod render;
mod rule;
mod sources;
mod validate;

use lighthouse_resource::Descriptor;

pub use catalog::{Catalog, write_atomic, write_atomic_guarded};
pub use check::{BuiltinCheck, CelCheck, Check, Select};
pub use decision::{
    Decision, DecisionSpec, LanguageSpec, MIGRATED_FROM, PACK_LABEL, SECTION_LABEL,
};
pub use error::Error;
pub use fix::{
    CommandOutput, CommandScope, CommandSpec, CommandStdin, Fix, FixKind, OpSpec, ReorderScope,
};
pub use fix_docs::{OPERATIONS, Operation, Param, fix_operations_markdown};
pub use migrate::{
    is_override, is_resource, migrate_decision, migrate_override, migrate_pack, migrate_sources,
};
pub use model::{
    Content, Domain, Enforcement, Example, ExampleFile, ExampleKind, Expect, Scope, Subject,
    needs_verdict, tier,
};
pub use options::{ObjectType, OptionSchema, OptionType, OptionsSchema};
pub use overrides::DecisionOverrideSpec;
pub use pack::{Pack, PackSpec, Section, SectionSpec};
pub use render::{DOCS_DIR, decision_markdown, docs, help_path};
pub use rule::DecisionRule;
pub use sources::{Source, SourceMapSpec};

/// The JSON Schema of every kind this crate defines.
pub fn descriptors() -> Vec<Descriptor> {
    vec![
        Descriptor::of::<DecisionSpec>(),
        Descriptor::of::<DecisionOverrideSpec>(),
        Descriptor::of::<PackSpec>(),
        Descriptor::of::<SourceMapSpec>(),
    ]
}
