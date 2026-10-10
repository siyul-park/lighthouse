mod catalog;
mod check;
mod decision;
mod error;
mod fix;
mod fix_docs;
mod legacy;
mod load;
mod local;
mod model;
mod options;
mod pack;
mod pack_docs;
mod project;
mod render;
mod sources;
mod validate;

use lighthouse_resource::Descriptor;

pub use catalog::{Catalog, write_atomic, write_atomic_guarded};
pub use check::{
    At, Batch, Binding, BuiltinCheck, BuiltinOp, CelCheck, Check, CheckKind, CheckStdin,
    CommandCheck, CycleLevel, DEFAULT_TIMEOUT, ExitCodes, Identity, ModelCheck, NamedRule,
    OrderClause, OrderReport, OrderScope, RpcCheck, Select, TemplatePart, parse_template,
};
pub use decision::{
    Decision, DecisionSpec, LanguageSpec, MIGRATED_FROM, PACK_LABEL, PRESET_LABEL, Provenance,
    SECTION_LABEL, STRICT, WAS_BUILTIN, WAS_ENFORCEMENT, WAS_EXCEPTIONS,
};
pub use error::Error;
pub use fix::{
    CommandOutput, CommandScope, CommandSpec, CommandStdin, Fix, FixKind, OpSpec, ReorderScope,
};
pub use fix_docs::{OPERATIONS, Operation, Param, fix_operations_markdown};
pub use legacy::from_legacy_type;
pub use local::{load_local, local_dir, local_files};
pub use model::{
    Content, DecisionStatus, Domain, Example, ExampleFile, ExampleKind, Expect, Scope, Subject,
    authored_severity,
};
pub use options::{ObjectType, OptionSchema, OptionType, OptionsSchema};
pub use pack::{Pack, PackSpec, Section, SectionSpec};
pub use project::{
    Config, FILE_NAME, FILE_NAMES, Formatter, FormatterOutput, FormatterSpec, FormatterStdin,
    GeneratedCheck, GeneratedSpec, GlobSet, Layer, Level, OverrideSpec, PluginEntry, PluginRef,
    PluginRefSpec, ProjectError, ProjectLanguage, ProjectSpec, Projects, RuleConfig, RuleDetail,
    RuleSetting, Rules, glob_set,
};
pub use render::{DOCS_DIR, decision_markdown, docs, help_path};
pub use sources::{SourceLine, SourceMapSpec};

/// The JSON Schema of every kind this crate defines.
pub fn descriptors() -> Vec<Descriptor> {
    vec![
        Descriptor::of::<DecisionSpec>(),
        Descriptor::of::<PackSpec>(),
        Descriptor::of::<ProjectSpec>(),
        Descriptor::of::<SourceMapSpec>(),
    ]
}
