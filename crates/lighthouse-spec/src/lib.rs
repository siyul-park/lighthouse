mod catalog;
mod check;
mod command;
mod decision;
mod error;
mod fix;
mod fix_docs;
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
    At, Binding, BuiltinCheck, BuiltinOp, CelCheck, Check, CheckKind, CycleLevel, DEFAULT_TIMEOUT,
    Identity, ModelCheck, NamedRule, OrderClause, OrderReport, OrderScope, RpcCheck, Select,
    TemplatePart, parse_template,
};
pub use command::{
    Batch, CheckOutput, CheckStdin, CommandCheck, ExitCodes, SarifColumns, SarifSelect,
};
pub use decision::{
    Decision, DecisionSpec, LanguageSpec, PACK_LABEL, PRESET_LABEL, Provenance, SECTION_LABEL,
    STRICT,
};
pub use error::Error;
pub use fix::{
    CommandOutput, CommandScope, CommandSpec, CommandStdin, Fix, FixKind, OpSpec, ReorderScope,
};
pub use fix_docs::{OPERATIONS, Operation, Param, fix_operations_markdown};
pub use local::{load_local, local_dir, local_files};
pub use model::{
    Content, DecisionStatus, Domain, Example, ExampleFile, ExampleKind, Expect, Scope, Subject,
    authored_severity,
};
pub use options::{
    LIMIT_ROLES, NO_LIMIT, ObjectType, OptionSchema, OptionType, OptionsSchema, Shape, definitions,
};
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
    let mut decision = Descriptor::of::<DecisionSpec>();
    // The named shapes a `$ref` of an option may point to.
    if let Some(defs) = decision
        .schema
        .get_mut("$defs")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (name, shape) in options::definitions() {
            defs.insert(
                name.clone(),
                serde_json::to_value(shape).expect("a shape serializes"),
            );
        }
    }
    vec![
        decision,
        Descriptor::of::<PackSpec>(),
        Descriptor::of::<ProjectSpec>(),
        Descriptor::of::<SourceMapSpec>(),
    ]
}
