mod catalog;
mod check;
mod command;
mod decision;
mod error;
mod expression;
mod fix;
mod model;
mod model_check;
mod options;
mod pack;
mod project;
mod sources;
mod union;
mod validate;

use lighthouse_resource::Descriptor;

pub use catalog::Catalog;
pub use catalog::load::{load_local, local_dir, local_files};
pub use check::{
    BuiltinCheck, BuiltinOp, CelCheck, Check, CheckKind, CycleLevel, DEFAULT_TIMEOUT, FORMATTED,
    NamedRule, OrderReport, OrderScope, Select,
};
pub use command::{Batch, CheckOutput, CommandCheck, ExitCodes, SarifColumns, SarifSelect};
pub use decision::{Decision, DecisionSpec, LanguageSpec, Provenance, authored_severity};
pub use error::Error;
pub use expression::{TemplatePart, parse_template};
pub use fix::{CommandSpec, Fix, FixKind, OpSpec};
pub use model::{DecisionStatus, Example, ExampleFile, ExampleKind, Expect, Scope, Subject};
pub use model_check::ModelCheck;
pub use options::{LIMIT_ROLES, OptionSchema, OptionType, OptionsSchema, Shape, limit};
pub use pack::{Pack, Section};
pub(crate) use pack::{PackSpec, SectionSpec};
pub use project::{
    Config, Evaluation, FILE_NAME, Formatter, GeneratedCheck, GlobSet, Level, ModelInput,
    ModelProvider, Models, OverrideSpec, PluginRef, ProjectSpec, Projects, RESERVED_PROVIDERS,
    Routing, RuleConfig, RuleSetting, Rules, glob_set,
};
pub(crate) use sources::SourceMapSpec;

/// The JSON Schema of every kind this crate defines.
pub fn descriptors() -> Vec<Descriptor> {
    let mut decision = Descriptor::new::<DecisionSpec>();
    // The named shape a `$ref` of an option may point to.
    if let Some(defs) = decision
        .schema
        .get_mut("$defs")
        .and_then(serde_json::Value::as_object_mut)
    {
        defs.insert(
            "limit".to_owned(),
            serde_json::to_value(limit()).expect("a shape serializes"),
        );
    }
    vec![
        decision,
        Descriptor::new::<PackSpec>(),
        Descriptor::new::<ProjectSpec>(),
        Descriptor::new::<SourceMapSpec>(),
    ]
}
