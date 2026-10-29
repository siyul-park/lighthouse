mod canonical;
pub mod hash;
mod language;
mod model;
mod route;
pub mod stats;

pub use canonical::canonical;
pub use language::{
    TextOffsets, coalesce_text_map,
    frame::{ErrorObject, FrameError, Id, Message, read_message, write_message},
    host::{HostError, HostLanguage, HostParser, HostTree, host_handler, serve_host},
    serve::{Handler, ServeError, serve},
    wire::{
        CacheRef, Call, Comment, Context, Conventions, Edge, EdgeKind, EmbeddedFile,
        EmbeddedSource, Event, EventKind, FileInfo, FileRef, Flow, FlowKind, Fragment,
        FunctionSummary, Implementation, Incomplete, IndexInput, IndexParams, IndexResult,
        InitializeParams, InitializeResult, Mapping, Methods, Module, Node, Overlay, Position,
        ProjectRef, ProviderManifest, Resolution, ServerCapabilities, Signature, Source, Span,
        Symbol, SymbolKind, SymbolRole, TestCase, TestNaming, TestStyle, TextMapSegment, TypeRef,
        Visibility, WorkspaceFolder, pointer_child, pointer_tokens, pointer_value,
    },
};
pub use model::{
    agent::{EditParams, EditResult, Tool, ToolEvent},
    jobs::{
        Capabilities, ChatMessage, CreateFineTuningJobRequest, FineTuneJobError, FineTuneMethod,
        FineTuneSupervisedHyperparameters, FineTuneSupervisedMethod, FineTuningJob,
        FineTuningJobStatus, LineKeys, ListModelResponse, Model, Supports, TrainingLine,
    },
    methods::{
        Answer, Criteria, Criterion, DecisionRef, Example, Instructions, ModelMethods, NOUL,
        Neighbor, Question, State, Subject, SystemOneRequest, SystemOneResponse,
    },
    prompt,
    store::{DATASETS_DIR, MODELS_DIR, artifact_path, content_dir, dataset_path},
};
pub use route::{FINE_TUNING_JOBS, Http, ROUTES, Route, SYSTEM_ONE};

/// Protocol version spoken by this crate. Peers must agree on it exactly.
pub const VERSION: &str = "0.4";

/// Request: handshake; the first message of a session.
pub const INITIALIZE: &str = "initialize";
/// Request: index the files of one language.
pub const INDEX: &str = "lighthouse/index";
/// Request: put the subjects of a decision to a model.
pub const CLASSIFY: &str = "lighthouse/classify";
/// Request: fit a model on labelled subjects.
pub const TRAIN: &str = "lighthouse/train";
/// Request: run an agent that changes the files of a directory.
pub const EDIT: &str = "lighthouse/edit";
/// Request: stop accepting work; answered, then followed by [`EXIT`].
pub const SHUTDOWN: &str = "shutdown";
/// Notification: end the process; valid only after [`SHUTDOWN`] was answered.
pub const EXIT: &str = "exit";

/// JSON Schema of every method's params and result, with the wire types under
/// `$defs`.
pub fn schema() -> serde_json::Value {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(Methods)).expect("schema serializes");
    let models =
        serde_json::to_value(schemars::schema_for!(ModelMethods)).expect("schema serializes");
    merge_models(&mut schema, &models);
    schema
}

/// Adds the model methods to the schema of the language methods: their
/// properties, and the types they define.
fn merge_models(schema: &mut serde_json::Value, models: &serde_json::Value) {
    for key in ["properties", "$defs"] {
        let (Some(into), Some(from)) = (
            schema.get_mut(key).and_then(|v| v.as_object_mut()),
            models.get(key).and_then(|v| v.as_object()),
        ) else {
            continue;
        };
        for (name, value) in from {
            into.entry(name.clone()).or_insert_with(|| value.clone());
        }
    }
}

pub use language::selector::{Selector, SelectorComparison, SelectorFilter, SelectorPart};
