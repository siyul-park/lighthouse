//! The spec kinds (decisions, packs, project configuration), the `Catalog`
//! that merges them, and the CEL environment their expressions run in. It
//! knows no plugins and no libraries.

mod attributes;
mod catalog;
mod check;
mod command;
mod decision;
mod edit;
mod env;
mod error;
mod example;
mod expression;
mod fix;
mod model_check;
mod options;
mod pack;
mod program;
mod project;
mod scope;
mod sources;
mod union;
mod validate;

use lighthouse_model::Schema;

pub use attributes::Attributes;
pub use catalog::Catalog;
pub use catalog::load::{Files, load_local, local_dir, local_files};
pub use check::{At, CelCheck, Check, CheckKind, Identity, Select, Variable};
pub use command::CommandCheck;
pub use decision::DecisionStatus;
pub use decision::{Decision, DecisionSpec, LanguageSpec, Provenance, authored_severity};
pub use edit::KIND as EDIT_KIND;
pub use env::{Env, base};
pub use error::Error;
pub use example::{Example, ExampleFile, ExampleKind, Expect};
pub use expression::{
    Call, SUBJECT_VARIABLES, TemplatePart, children, free_variables, identifiers, parse_template,
    program_calls, program_identifiers,
};
pub use fix::{CommandSpec, Fix, FixKind, FixScope};
pub use model_check::ModelCheck;
pub use options::{LIMIT_ROLES, OptionSchema, OptionType, OptionsSchema, Shape, limit};
pub use pack::{Pack, Section};
pub(crate) use pack::{PackSpec, SectionSpec};
pub use program::{
    Batching, DEFAULT_TIMEOUT, LONG_TIMEOUT, Output, Point, Program, ProgramSpec, Programs,
    Sandbox, Sarif, SarifColumns, Stdin, file_fact,
};
pub use project::{
    BUNDLED, CommandProtocol, Config, Entry, EnvironmentRule, EnvironmentSpec, Evaluation,
    EventsSpec, FILE_NAME, FixEnvironment, FixPolicy, FixSpec, GeneratedCheck, GlobSet, Layer,
    Layers, Level, ModelProvider, Models, OverrideSpec, PluginRef, ProjectSpec, Projects,
    RESERVED_PROVIDERS, Routing, RuleConfig, RuleSetting, Rules, SandboxSpec, SignalsSpec,
    SourceLanguageSpec, SourceRuleSpec, Surface, Task, TestNamingSpec, glob_set,
};
pub use scope::{Scope, Subject};
pub(crate) use sources::SourceMapSpec;

/// The JSON Schema of every kind this crate defines.
pub fn descriptors() -> Vec<Schema> {
    let mut decision = Schema::new::<DecisionSpec>();
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
        Schema::bare::<EnvironmentSpec>(),
        Schema::new::<PackSpec>(),
        Schema::new::<ProjectSpec>(),
        Schema::new::<SourceMapSpec>(),
    ]
}
