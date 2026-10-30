//! The agent-loop commands as tools: one table says which command each tool
//! is the twin of, what it returns, what it may do to the world and how it
//! runs. Titles and descriptions come from the command's own help; the
//! schemas of its arguments and of its result from the structs that hold the
//! command's flags and the data it prints.

use std::collections::BTreeMap;

use lighthouse_model::{SCHEMA_URL_BASE, schema_file};
use lighthouse_session::{
    Authored, CheckReport, Session,
    fix::Fixed,
    model::{Attachment, Detachment, EvalReport, Rebuilt, Trained, remote_bound},
    review::{Recorded, Shown},
    signal::{Signal, SignalReport},
};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::surface::{
    Result,
    args::{
        CheckArgs, ConfigShowArgs, DecisionEvalArgs, DecisionListArgs, DecisionShowArgs,
        DecisionSignalsArgs, DecisionTestArgs, DocumentArgs, FingerprintArgs, FixArgs,
        ModelAttachArgs, ModelDetachArgs, ModelListArgs, ModelTrainArgs, ReviewListArgs,
        ReviewResolveArgs, SignalDismissArgs,
    },
    check,
    commands::{self, CheckCommand},
    config::{self, ConfigShow},
    decision::{self, DecisionList, DecisionShow, DecisionTested},
    model::{self, ModelList},
    reply::{CheckFormat, Context, Format, Reply},
    review::{self, JudgmentHistory, ReviewList},
};

/// What the tool `fix` is: the twin of `check --fix`, as its own tool because
/// it edits files.
const FIX_TITLE: &str = "Fix findings with the fixers their decisions name.";
const FIX_DESCRIPTION: &str = "Fix findings with the fixers the decisions name (`fix` in `decision_list`: who makes it, `safe` or `unsafe`, whether it runs). Select what to fix with `fingerprints` (findings from `check`, prefixes of at least 7 characters allowed), `paths` or `decision`; at least one is required. A fix is proposed, applied, re-checked (the safe fixes of what it introduced follow it) and rolled back for any file that gains an error or stops being analyzed, and repeated until nothing is left (at most 5 rounds). Only `safe` fixes are written, whatever makes them; an `unsafe` one is returned as a preview with its diff and the call that applies exactly it (`fingerprints` of every finding of its run, with `unsafeFixes`), unless `unsafeFixes` is true or, when it is not set, the project's `[fix] unsafe` is true; `unsafeFixes: false` only previews. Applying a preview writes exactly the edit that was shown, kept by the run that made it: a file that changed since gives a conflict and writes nothing, and a preview that is not kept is declined. `noModels` asks no model. Use `dryRun` first to see the unified `diff` without changing a file. Returns the `diff`, the `applied` fixes, the `previews` and the `declined` findings with the reason each was left alone; recheck with `check` afterwards. Findings a judgment hides are never fixed.";

/// The kind of what `model rebuild` prints, which is no tool.
const REBUILD_KIND: &str = "ModelRebuild";

/// The tools, in the order they are listed.
pub const TOOLS: [Tool; 19] = [
    Tool::new::<CheckArgs, CheckReport>(
        "check",
        &["check"],
        "CheckReport",
        Hints::WRITE.open(),
        |ctx, args| {
            let command = CheckCommand {
                args: parse(args)?,
                format: CheckFormat::Json,
                ..CheckCommand::default()
            };
            check::check(ctx, &command)
        },
    ),
    Tool::new::<FixArgs, Fixed>(
        "fix",
        &["check"],
        "FixReport",
        Hints::WRITE.destructive().open(),
        |ctx, args| check::fix(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<DecisionListArgs, DecisionList>(
        "decision_list",
        &["decision", "list"],
        "DecisionList",
        Hints::READ,
        |ctx, args| decision::list(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<DecisionShowArgs, DecisionShow>(
        "decision_show",
        &["decision", "show"],
        "DecisionShow",
        Hints::READ,
        |ctx, args| decision::show(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<DocumentArgs, Authored>(
        "decision_create",
        &["decision", "create"],
        "DecisionReport",
        Hints::WRITE.repeatable(),
        |ctx, args| decision::create(ctx, &parse::<DocumentArgs>(args)?.document, Format::Json),
    ),
    Tool::new::<DocumentArgs, Authored>(
        "decision_update",
        &["decision", "update"],
        "DecisionReport",
        Hints::WRITE,
        |ctx, args| decision::update(ctx, &parse::<DocumentArgs>(args)?.document, Format::Json),
    ),
    Tool::new::<DecisionSignalsArgs, SignalReport>(
        "decision_signals",
        &["decision", "signals"],
        "DecisionSignals",
        Hints::READ,
        |ctx, args| decision::signals(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<SignalDismissArgs, Signal>(
        "decision_signal_dismiss",
        &["decision", "signal", "dismiss"],
        "SignalDismiss",
        Hints::WRITE,
        |ctx, args| decision::dismiss(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<DecisionTestArgs, DecisionTested>(
        "decision_test",
        &["decision", "test"],
        "DecisionTest",
        Hints::READ,
        |ctx, args| decision::test(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<DecisionEvalArgs, EvalReport>(
        "decision_eval",
        &["decision", "eval"],
        "DecisionEval",
        Hints::READ.open(),
        |ctx, args| decision::eval(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<ReviewListArgs, ReviewList>(
        "review_list",
        &["review", "list"],
        "ReviewList",
        Hints::READ,
        |ctx, args| review::list(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<FingerprintArgs, Shown>(
        "review_show",
        &["review", "show"],
        "Finding",
        Hints::READ,
        |ctx, args| review::show(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<ReviewResolveArgs, Recorded>(
        "review_resolve",
        &["review", "resolve"],
        "ReviewResolve",
        Hints::WRITE,
        |ctx, args| review::resolve(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<FingerprintArgs, JudgmentHistory>(
        "review_history",
        &["review", "history"],
        "JudgmentHistory",
        Hints::READ,
        |ctx, args| review::history(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<ModelListArgs, ModelList>(
        "model_list",
        &["model", "list"],
        "ModelList",
        Hints::READ,
        |ctx, args| model::list(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<ModelTrainArgs, Trained>(
        "model_train",
        &["model", "train"],
        "ModelTrain",
        Hints::WRITE.repeatable().open(),
        |ctx, args| model::train(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<ModelAttachArgs, Attachment>(
        "model_attach",
        &["model", "attach"],
        "ModelAttach",
        Hints::WRITE.open(),
        |ctx, args| model::attach(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<ModelDetachArgs, Detachment>(
        "model_detach",
        &["model", "detach"],
        "ModelDetach",
        Hints::WRITE,
        |ctx, args| model::detach(ctx, &parse(args)?, Format::Json),
    ),
    Tool::new::<ConfigShowArgs, ConfigShow>(
        "config_show",
        &["config", "show"],
        "ConfigShow",
        Hints::READ,
        |ctx, args| config::show(ctx, &parse(args)?, Format::Json),
    ),
];

/// How a tool may touch the world, in the MCP annotations' terms.
#[derive(Debug, Clone, Copy)]
pub struct Hints {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    /// Whether it may reach outside the machine: when a remote model is bound.
    pub open_world: bool,
}

impl Hints {
    /// Changes nothing.
    const READ: Self = Self {
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    };
    /// Writes, and doing it again changes nothing more.
    const WRITE: Self = Self {
        read_only: false,
        ..Self::READ
    };

    /// Also overwrites what was there.
    const fn destructive(self) -> Self {
        Self {
            destructive: true,
            ..self
        }
    }

    /// Doing it again changes more.
    const fn repeatable(self) -> Self {
        Self {
            idempotent: false,
            ..self
        }
    }

    /// Also reaches outside the machine when a remote model is bound.
    const fn open(self) -> Self {
        Self {
            open_world: true,
            ..self
        }
    }
}

/// An agent-loop command as a tool.
#[derive(Debug, Clone, Copy)]
pub struct Tool {
    /// The command path joined by `_`; `fix` is the tool of `check --fix`.
    pub name: &'static str,
    /// The command it is the twin of.
    pub path: &'static [&'static str],
    /// The output kind: its schema is the tool's `outputSchema`.
    pub kind: &'static str,
    pub hints: Hints,
    input: fn() -> Value,
    output: fn() -> Value,
    call: fn(&Context, Value) -> Result<Reply>,
}

impl Tool {
    /// A tool of the table: its arguments `A` and the data `D` it returns.
    const fn new<A: JsonSchema, D: JsonSchema>(
        name: &'static str,
        path: &'static [&'static str],
        kind: &'static str,
        hints: Hints,
        call: fn(&Context, Value) -> Result<Reply>,
    ) -> Self {
        Self {
            name,
            path,
            kind,
            hints,
            input: input::<A>,
            output: output::<D>,
            call,
        }
    }

    /// The JSON Schema of the arguments, from the struct of the flags: an
    /// object that takes no other.
    pub fn input_schema(&self) -> Value {
        let mut schema = (self.input)();
        schema["additionalProperties"] = Value::Bool(false);
        schema
    }

    /// The JSON Schema of the result, as `lighthouse schema` prints it.
    pub fn output_schema(&self) -> Value {
        document(self.kind, (self.output)())
    }

    /// The one-line summary and the long help of the command.
    pub fn help(&self) -> (String, String) {
        if self.name == "fix" {
            return (FIX_TITLE.to_owned(), FIX_DESCRIPTION.to_owned());
        }
        commands::help(self.path)
    }

    /// Whether the tool may reach outside the machine in `session`'s project.
    pub fn open_world(&self, session: &Session) -> bool {
        self.hints.open_world && remote_bound(session)
    }
}

/// Every tool, in listing order.
pub fn tools() -> &'static [Tool] {
    &TOOLS
}

/// Every output kind with its schema, by kind name: what the tools return,
/// and what `model rebuild` prints.
pub fn kinds() -> BTreeMap<&'static str, Value> {
    let rebuild = (
        REBUILD_KIND,
        document(REBUILD_KIND, output::<Vec<Rebuilt>>()),
    );
    tools()
        .iter()
        .map(|tool| (tool.kind, tool.output_schema()))
        .chain([rebuild])
        .collect()
}

/// The schema of the arguments `T` a tool takes: what it may be given.
pub fn input<T: JsonSchema>() -> Value {
    describe::<T>(SchemaSettings::draft2020_12())
}

/// Runs the tool `name` with `arguments`. `None` when there is no such tool;
/// an error when the arguments are not those of its command or the command
/// fails. The reply is always JSON.
pub fn call(ctx: &Context, name: &str, arguments: Value) -> Option<Result<Reply>> {
    let tool = tools().iter().find(|t| t.name == name)?;
    Some(dispatch(ctx, tool, arguments))
}

/// The schema of the result `T` a tool returns: what it always prints. The
/// objects of an untagged enum are alternatives, and MCP wants the root to say
/// it is an object.
fn output<T: JsonSchema>() -> Value {
    let mut schema = describe::<T>(SchemaSettings::draft2020_12().for_serialize());
    if schema.get("anyOf").is_some() && schema.get("type").is_none() {
        schema["type"] = json!("object");
    }
    schema
}

/// The schema of `T` with its subschemas inline.
fn describe<T: JsonSchema>(settings: SchemaSettings) -> Value {
    let generator = settings
        .with(|settings| settings.inline_subschemas = true)
        .into_generator();
    let mut schema =
        serde_json::to_value(generator.into_root_schema_for::<T>()).expect("a schema serializes");
    if let Some(map) = schema.as_object_mut() {
        map.remove("$schema");
        map.remove("title");
    }
    schema
}

/// The schema of a kind of output as a document: with its `$id` and title.
fn document(kind: &str, mut schema: Value) -> Value {
    schema["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
    schema["$id"] = json!(format!("{SCHEMA_URL_BASE}/{}", schema_file(kind)));
    schema["title"] = json!(kind);
    schema
}

fn dispatch(ctx: &Context, tool: &Tool, arguments: Value) -> Result<Reply> {
    let arguments = if arguments.is_null() {
        Value::Object(Default::default())
    } else {
        arguments
    };
    known_keys(tool, &arguments)?;
    (tool.call)(ctx, arguments)
}

/// Refuses an argument the tool does not have, naming it: the structs that
/// flatten others cannot say so themselves.
fn known_keys(tool: &Tool, arguments: &Value) -> Result<()> {
    let schema = tool.input_schema();
    let known = schema.get("properties").and_then(Value::as_object);
    let given = arguments
        .as_object()
        .ok_or("invalid arguments: expected an object")?;
    for key in given.keys() {
        if !known.is_some_and(|known| known.contains_key(key)) {
            return Err(format!("invalid arguments: unknown field `{key}`").into());
        }
    }
    Ok(())
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|e| format!("invalid arguments: {e}").into())
}
