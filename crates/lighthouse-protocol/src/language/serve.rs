use std::{
    io::{BufRead, Write},
    panic::{AssertUnwindSafe, catch_unwind},
};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use super::frame::{
    INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND, PARSE_ERROR, SERVER_NOT_INITIALIZED,
};
use crate::{
    CHAT, CLASSIFY, ChatRequest, CreateChatCompletionResponse, CreateFineTuningJobRequest, EXIT,
    ErrorObject, FineTuningJob, FrameError, INDEX, INITIALIZE, IndexParams, IndexResult,
    InitializeParams, InitializeResult, Message, SHUTDOWN, SystemOneRequest, SystemOneResponse,
    TRAIN, VERSION, read_message, write_message,
};

/// The methods of a plugin. An `Err` becomes a failed-request response; the
/// plugin stays usable. [`serve`] refuses an `initialize` of another protocol
/// version before the handler sees it.
///
/// A handler that panics is answered with an internal error and the loop
/// carries on, but the handler's state may then be inconsistent (a panic can
/// leave a half-updated cache or index). A host should treat the batch as
/// incomplete and may restart the plugin rather than trust later answers.
pub trait Handler {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, ErrorObject>;
    /// Indexes the files of one language; a plugin that indexes none has no
    /// fragment to report.
    fn index(&mut self, _params: IndexParams) -> Result<IndexResult, ErrorObject> {
        Ok(IndexResult::default())
    }
    /// Puts subjects to a model; a language plugin offers no model.
    fn classify(&mut self, _params: SystemOneRequest) -> Result<SystemOneResponse, ErrorObject> {
        Err("this plugin offers no model".into())
    }
    /// Has a model reply to a conversation; only a model that generates text
    /// offers it.
    fn chat(&mut self, _params: ChatRequest) -> Result<CreateChatCompletionResponse, ErrorObject> {
        Err("this plugin does not chat".into())
    }
    /// Fits a model; only a model that learns from labels offers it.
    fn train(&mut self, _params: CreateFineTuningJobRequest) -> Result<FineTuningJob, ErrorObject> {
        Err("this plugin does not train".into())
    }
}

/// Why [`serve`] ended without a clean `shutdown` and `exit`.
#[derive(Debug, Error)]
pub enum ServeError {
    #[error("input closed before exit")]
    Closed,
    #[error("exit before shutdown")]
    ExitWithoutShutdown,
    #[error(transparent)]
    Frame(#[from] FrameError),
}

/// What the loop knows of the conversation.
#[derive(Default)]
struct Session {
    initialized: bool,
    shutdown: bool,
}

/// Answers requests from `reader` on `writer` until the host sends `exit`.
/// Returns `Ok` after a `shutdown` followed by `exit`, or by the input closing
/// once `shutdown` was answered (a host that went away after asking the plugin
/// to stop). Input that closes before `shutdown` is [`ServeError::Closed`]; a
/// malformed message is answered with a parse error and ends the loop. A
/// request other than `initialize` before a successful one is refused. The
/// loop knows framing and method names only, nothing about the code model.
pub fn serve(
    mut reader: impl BufRead,
    mut writer: impl Write,
    handler: &mut impl Handler,
) -> Result<(), ServeError> {
    let mut session = Session::default();
    loop {
        let mut message = match read_message(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) if session.shutdown => return Ok(()),
            Ok(None) => return Err(ServeError::Closed),
            Err(error) => {
                let failure =
                    Message::failure(None, ErrorObject::new(PARSE_ERROR, error.to_string()));
                write_message(&mut writer, &failure)?;
                return Err(error.into());
            }
        };
        let Some(method) = message.method.take() else {
            continue;
        };
        if method == EXIT {
            return if session.shutdown {
                Ok(())
            } else {
                Err(ServeError::ExitWithoutShutdown)
            };
        }
        let id = message.id.take();
        let outcome = dispatch(handler, &method, message, &mut session);
        if let Some(id) = id {
            let reply = match outcome {
                Ok(value) => Message::response(id, value).expect("a value serializes"),
                Err(error) => Message::failure(Some(id), error),
            };
            write_message(&mut writer, &reply)?;
        }
    }
}

fn dispatch(
    handler: &mut impl Handler,
    method: &str,
    message: Message,
    session: &mut Session,
) -> Result<Value, ErrorObject> {
    catch_unwind(AssertUnwindSafe(|| match method {
        INITIALIZE => {
            let result = call(message, |p| initialize(handler, p));
            session.initialized |= result.is_ok();
            result
        }
        _ if !session.initialized => Err(ErrorObject::new(
            SERVER_NOT_INITIALIZED,
            format!("`{method}` before `initialize`"),
        )),
        INDEX => call(message, |p| handler.index(p)),
        CLASSIFY => call(message, |p| handler.classify(p)),
        CHAT => call(message, |p| handler.chat(p)),
        TRAIN => call(message, |p| handler.train(p)),
        SHUTDOWN => {
            session.shutdown = true;
            Ok(Value::Null)
        }
        other => Err(ErrorObject::new(
            METHOD_NOT_FOUND,
            format!("unknown method {other}"),
        )),
    }))
    .unwrap_or_else(|panic| {
        let text = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown".to_owned());
        Err(ErrorObject::new(INTERNAL_ERROR, format!("panic: {text}")))
    })
}

/// The handshake: peers must speak the same protocol version exactly.
fn initialize(
    handler: &mut impl Handler,
    params: InitializeParams,
) -> Result<InitializeResult, ErrorObject> {
    if params.protocol_version != VERSION {
        return Err(format!(
            "host speaks protocol {}, this plugin speaks {VERSION}",
            params.protocol_version
        )
        .into());
    }
    handler.initialize(params)
}

fn call<P: serde::de::DeserializeOwned, R: Serialize>(
    message: Message,
    run: impl FnOnce(P) -> Result<R, ErrorObject>,
) -> Result<Value, ErrorObject> {
    let params = message
        .into_params::<P>()
        .map_err(|e| ErrorObject::new(INVALID_PARAMS, e.to_string()))?;
    serde_json::to_value(run(params)?).map_err(|e| ErrorObject::new(INTERNAL_ERROR, e.to_string()))
}
