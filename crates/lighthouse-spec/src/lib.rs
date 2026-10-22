mod attributes;
mod catalog;
mod check;
mod command;
mod decision;
mod edit;
mod environment;
mod error;
mod expression;
mod ext;
mod fix;
mod model;
mod model_check;
mod options;
mod pack;
mod program;
mod project;
mod sources;
mod union;
mod validate;

use lighthouse_resource::Descriptor;

pub use attributes::Attributes;
pub use catalog::Catalog;
pub use catalog::load::{load_local, local_dir, local_files};
pub use check::{
    BuiltinCheck, BuiltinOp, CelCheck, Check, CheckKind, CycleLevel, NamedRule, Select, Variable,
};
pub use command::CommandCheck;
pub use decision::{Decision, DecisionSpec, LanguageSpec, Provenance, authored_severity};
pub use edit::KIND as EDIT_KIND;
pub use environment::{compile, compile_shared, context, environment};
pub use error::Error;
pub use expression::{
    Call, SUBJECT_VARIABLES, TemplatePart, children, free_variables, identifiers, parse_template,
    program_calls, program_identifiers,
};
pub use fix::{CommandSpec, Fix, FixKind, FixScope};
pub use model::{DecisionStatus, Example, ExampleFile, ExampleKind, Expect, Scope, Subject};
pub use model_check::ModelCheck;
pub use options::{LIMIT_ROLES, OptionSchema, OptionType, OptionsSchema, Shape, limit};
pub use pack::{Pack, Section};
pub(crate) use pack::{PackSpec, SectionSpec};
pub use program::{
    Batching, DEFAULT_TIMEOUT, LONG_TIMEOUT, Output, Point, Program, ProgramSpec, Sandbox, Sarif,
    SarifColumns, Stdin,
};
pub use project::{
    BUNDLED, CommandProtocol, Config, Entry, EnvironmentRule, EnvironmentSpec, Evaluation,
    EventsSpec, FILE_NAME, FixEnvironment, FixPolicy, FixSpec, GeneratedCheck, GlobSet, Layer,
    Layers, Level, ModelProvider, Models, OverrideSpec, PluginRef, ProjectSpec, Projects,
    RESERVED_PROVIDERS, Routing, RuleConfig, RuleSetting, Rules, SandboxSpec, Task, glob_set,
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
        Descriptor::bare::<EnvironmentSpec>(),
        Descriptor::new::<PackSpec>(),
        Descriptor::new::<ProjectSpec>(),
        Descriptor::new::<SourceMapSpec>(),
    ]
}
