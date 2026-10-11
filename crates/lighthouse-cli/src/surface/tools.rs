//! The agent-loop commands as tools: one table says which command each tool
//! is the twin of, what it returns and what it may do to the world. Titles
//! and descriptions come from the command's own help; input schemas from the
//! arguments struct that also holds the command's flags.

use lighthouse_session::{Session, remote_bound};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::surface::{
    Result,
    args::{
        CheckArgs, DecisionEvalArgs, DecisionListArgs, DecisionShowArgs, DecisionTestArgs,
        DocumentArgs, FingerprintArgs, FixArgs, ModelListArgs, ModelTrainArgs, ReviewListArgs,
        ReviewResolveArgs,
    },
    check, cli, decision, eval, fix, model,
    reply::{CheckFormat, Context, Output, Reply},
    review,
};

/// What the tool `fix` is: the twin of `check --fix`, as its own tool because
/// it edits files.
const FIX_TITLE: &str = "Fix findings with the fixers their decisions name.";
const FIX_DESCRIPTION: &str = "Fix findings with the fixers the decisions name (`fixable: safe|suggested` in `decision_list`). Select what to fix with `fingerprints` (findings from `check`, prefixes of at least 7 characters allowed), `paths` or `decision`; at least one is required. A fix is proposed as edits over the code model, applied, formatted, re-checked and rolled back for any file that gains an error or stops being analyzed, and repeated until nothing is left (at most 5 rounds). Only safe fixes of mechanical rules are applied unless `unsafeFixes` is set. Use `dryRun` first to see the unified `diff` without changing a file. Returns the `diff`, the `applied` fixes and the `declined` findings with the reason each was left alone; recheck with `check` afterwards. Findings a judgment hides are never fixed.";

const READ: Hints = Hints {
    read_only: true,
    destructive: false,
    idempotent: true,
    open_world: false,
};

/// The tools, in the order they are listed.
pub const TOOLS: [Tool; 14] = [
    Tool::new(
        "check",
        &["check"],
        "CheckReport",
        Hints {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: true,
        },
        schema::<CheckArgs>,
    ),
    Tool::new(
        "fix",
        &["check"],
        "FixReport",
        Hints {
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        },
        schema::<FixArgs>,
    ),
    Tool::new(
        "decision_list",
        &["decision", "list"],
        "DecisionList",
        READ,
        schema::<DecisionListArgs>,
    ),
    Tool::new(
        "decision_show",
        &["decision", "show"],
        "DecisionShow",
        READ,
        schema::<DecisionShowArgs>,
    ),
    Tool::new(
        "decision_create",
        &["decision", "create"],
        "AuthorReport",
        Hints {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        schema::<DocumentArgs>,
    ),
    Tool::new(
        "decision_update",
        &["decision", "update"],
        "AuthorReport",
        Hints {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema::<DocumentArgs>,
    ),
    Tool::new(
        "decision_test",
        &["decision", "test"],
        "TestReport",
        READ,
        schema::<DecisionTestArgs>,
    ),
    Tool::new(
        "decision_eval",
        &["decision", "eval"],
        "EvalReport",
        Hints {
            open_world: true,
            ..READ
        },
        schema::<DecisionEvalArgs>,
    ),
    Tool::new(
        "review_list",
        &["review", "list"],
        "ReviewList",
        READ,
        schema::<ReviewListArgs>,
    ),
    Tool::new(
        "review_show",
        &["review", "show"],
        "Finding",
        READ,
        schema::<FingerprintArgs>,
    ),
    Tool::new(
        "review_resolve",
        &["review", "resolve"],
        "ResolveReport",
        Hints {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema::<ReviewResolveArgs>,
    ),
    Tool::new(
        "review_history",
        &["review", "history"],
        "JudgmentHistory",
        READ,
        schema::<FingerprintArgs>,
    ),
    Tool::new(
        "model_list",
        &["model", "list"],
        "ModelList",
        READ,
        schema::<ModelListArgs>,
    ),
    Tool::new(
        "model_train",
        &["model", "train"],
        "TrainReport",
        Hints {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        },
        schema::<ModelTrainArgs>,
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
}

impl Tool {
    /// A tool of the table.
    const fn new(
        name: &'static str,
        path: &'static [&'static str],
        kind: &'static str,
        hints: Hints,
        input: fn() -> Value,
    ) -> Self {
        Self {
            name,
            path,
            kind,
            hints,
            input,
        }
    }

    /// The JSON Schema of the arguments, from the struct of the flags.
    pub fn input_schema(&self) -> Value {
        (self.input)()
    }

    /// The one-line summary and the long help of the command.
    pub fn help(&self) -> (String, String) {
        if self.name == "fix" {
            return (FIX_TITLE.to_owned(), FIX_DESCRIPTION.to_owned());
        }
        cli::help(self.path)
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

/// The schema of a struct of arguments as a tool's `inputSchema`: an object
/// with the subschemas inline.
pub fn schema<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true)
        .into_generator();
    let mut schema =
        serde_json::to_value(generator.into_root_schema_for::<T>()).expect("a schema serializes");
    if let Some(map) = schema.as_object_mut() {
        map.remove("$schema");
        map.remove("title");
        map.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
    schema
}

/// Runs the tool `name` with `arguments`. `None` when there is no such tool;
/// an error when the arguments are not those of its command or the command
/// fails. The reply is always JSON: its `value` is the `structuredContent`.
pub fn call(ctx: &Context, name: &str, arguments: Value) -> Option<Result<Reply>> {
    let tool = tools().iter().find(|t| t.name == name)?;
    Some(dispatch(ctx, tool, arguments))
}

/// The long flag `--attributed-to` as the argument `attributedTo`.
pub fn camel_case(flag: &str) -> String {
    let mut name = String::new();
    for (n, word) in flag.split('-').enumerate() {
        if n == 0 {
            name.push_str(word);
        } else {
            let mut letters = word.chars();
            name.extend(letters.next().map(|c| c.to_ascii_uppercase()));
            name.push_str(letters.as_str());
        }
    }
    name
}

fn dispatch(ctx: &Context, tool: &Tool, arguments: Value) -> Result<Reply> {
    let arguments = if arguments.is_null() {
        Value::Object(Default::default())
    } else {
        arguments
    };
    known_keys(tool, &arguments)?;
    let arguments = with_env(tool, arguments);
    let json = Output::Json;
    match tool.name {
        "check" => check::run(
            ctx,
            &parse::<CheckArgs>(arguments)?,
            &check::Printing {
                format: CheckFormat::Json,
                ..check::Printing::default()
            },
        ),
        "fix" => fix::run(ctx, &parse::<FixArgs>(arguments)?, json),
        "decision_list" => decision::list(ctx, &parse(arguments)?, json),
        "decision_show" => decision::show(ctx, &parse(arguments)?, json),
        "decision_create" => {
            decision::author(ctx, &parse::<DocumentArgs>(arguments)?.document, true, json)
        }
        "decision_update" => decision::author(
            ctx,
            &parse::<DocumentArgs>(arguments)?.document,
            false,
            json,
        ),
        "decision_test" => decision::test(ctx, &parse(arguments)?, json),
        "decision_eval" => eval::run(ctx, &parse(arguments)?, json),
        "review_list" => review::list(&parse(arguments)?, json),
        "review_show" => review::show(&parse(arguments)?, json),
        "review_resolve" => review::resolve(ctx, &parse(arguments)?, json),
        "review_history" => review::history(&parse(arguments)?, json),
        "model_list" => model::list(ctx, &parse(arguments)?, json),
        "model_train" => model::train_model(ctx, &parse(arguments)?, json),
        other => Err(format!("unknown tool `{other}`").into()),
    }
}

/// The arguments with those the caller left out filled from the environment
/// variables the flags of the command read (`LIGHTHOUSE_ATTRIBUTED_TO`), so
/// that a tool has the defaults of its command.
fn with_env(tool: &Tool, mut arguments: Value) -> Value {
    let schema = tool.input_schema();
    let command = cli::command_at(tool.path);
    let Some(given) = arguments.as_object_mut() else {
        return arguments;
    };
    for arg in command.get_arguments() {
        let (Some(long), Some(env)) = (arg.get_long(), arg.get_env()) else {
            continue;
        };
        let name = camel_case(long);
        let text = schema["properties"][&name]["type"] == "string";
        let value = std::env::var(env).ok().filter(|value| !value.is_empty());
        if let (true, Some(value), false) = (text, value, given.contains_key(&name)) {
            given.insert(name, Value::String(value));
        }
    }
    arguments
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
