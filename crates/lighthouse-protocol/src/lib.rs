mod frame;
mod model;
mod prompt;
mod serve;
mod wire;

pub use frame::{ErrorObject, FrameError, Id, Message, read_message, write_message};
pub use model::{
    Capabilities, ClassifyParams, ClassifyResult, Criteria, Criterion, DecisionRef, Example,
    FineTuningJob, FineTuningJobRequest, Instructions, JobError, JobStatus, LabelSource, Method,
    ModelEntry, ModelList, ModelManifest, ModelMethods, Neighbor, Question, QuestionAnswer,
    ResultKind, Shot, State, Subject, SubjectContext, Supervised, Supports, TrainLine, TrainParams,
    TrainResponse, TrainSubject, training_file,
};
pub use prompt::{
    json_in, noul_of, prompt, response_schema, subject as subject_text, system as system_text,
};
pub use serve::{Handler, ServeError, serve};
pub use wire::{
    CacheRef, Call, ClientInfo, Comment, Context, Conventions, Edge, EdgeKind, Event, EventKind,
    FileInfo, FileRef, Flow, FlowKind, Fragment, FunctionSummary, Incomplete, IndexParams,
    IndexResult, InitializeParams, InitializeResult, Methods, Module, Node, Overlay, Position,
    ProjectRef, ProviderManifest, Resolution, Signature, Span, Symbol, SymbolKind, SymbolRole,
    TestCase, TestStyle, TypeRef, Visibility,
};

/// Protocol version spoken by this crate. Peers must agree on it exactly.
pub const VERSION: &str = "0.1";

/// Request: handshake; the first message of a session.
pub const INITIALIZE: &str = "initialize";
/// Request: index the files of one language.
pub const INDEX: &str = "index";
/// Request: put the subjects of a decision to a model.
pub const CLASSIFY: &str = "lighthouse/classify";
/// Request: fit a model on labelled subjects.
pub const TRAIN: &str = "lighthouse/train";
/// Request: stop accepting work; answered, then followed by [`EXIT`].
pub const SHUTDOWN: &str = "shutdown";
/// Notification: end the process; valid only after [`SHUTDOWN`] was answered.
pub const EXIT: &str = "exit";

/// Capability: `calls`, `references`, `implements` and `accesses-private`
/// edges come from the language's own semantic analysis.
pub const SEMANTIC_EDGES: &str = "semantic-edges";
/// Capability: every symbol carries its `extent`, the full declaration range.
pub const EXTENT: &str = "extent";
/// Capability: the provider analyzes the `overlays` of an index request instead
/// of the files on disk. A host declines fixes in files of a provider without it.
pub const OVERLAYS: &str = "overlays";
/// Capability: every reference to a symbol is reported as an edge with a
/// `site`, signatures, fields and receivers included. No bundled provider
/// declares it yet.
pub const COMPLETE_REFERENCES: &str = "complete-references";
/// Capability: edges carry the `site` where the reference occurs.
pub const REFERENCE_SITES: &str = "reference-sites";

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
