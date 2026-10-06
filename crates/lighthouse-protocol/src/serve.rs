use std::{
    io::{BufRead, Write},
    panic::{AssertUnwindSafe, catch_unwind},
};

use serde::Serialize;
use thiserror::Error;

use crate::{
    EXIT, FrameError, INDEX, INITIALIZE, IndexParams, IndexResult, InitializeParams,
    InitializeResult, Message, SHUTDOWN, read_message, write_message,
};

const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// The methods of a language provider. An `Err` becomes an internal error
/// response; the provider stays usable.
///
/// A handler that panics is answered with an internal error and the loop
/// carries on, but the handler's state may then be inconsistent (a panic can
/// leave a half-updated cache or index). A host should treat the batch as
/// incomplete and may restart the plugin rather than trust later answers.
pub trait Handler {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, String>;
    fn index(&mut self, params: IndexParams) -> Result<IndexResult, String>;
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

/// Answers requests from `reader` on `writer` until the host sends `exit`.
/// Returns `Ok` after a `shutdown` followed by `exit`, or by the input closing
/// once `shutdown` was answered (a host that went away after asking the plugin
/// to stop). Input that closes before `shutdown` is [`ServeError::Closed`]; a
/// malformed message is answered with a parse error and ends the loop. The loop knows framing and
/// method names only, nothing about the code model.
pub fn serve(
    mut reader: impl BufRead,
    mut writer: impl Write,
    handler: &mut impl Handler,
) -> Result<(), ServeError> {
    let mut shutdown = false;
    loop {
        let message = match read_message(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) if shutdown => return Ok(()),
            Ok(None) => return Err(ServeError::Closed),
            Err(error) => {
                let failure = Message::failure(None, PARSE_ERROR, error.to_string());
                write_message(&mut writer, &failure)?;
                return Err(error.into());
            }
        };
        let Some(method) = message.method.clone() else {
            continue;
        };
        if method == EXIT {
            return if shutdown {
                Ok(())
            } else {
                Err(ServeError::ExitWithoutShutdown)
            };
        }
        let id = message.id.clone();
        let reply = dispatch(handler, &method, message, &mut shutdown);
        if let Some(id) = id {
            write_message(&mut writer, &reply(id))?;
        }
    }
}

type Reply = Box<dyn FnOnce(crate::Id) -> Message>;

fn dispatch(
    handler: &mut impl Handler,
    method: &str,
    message: Message,
    shutdown: &mut bool,
) -> Reply {
    let outcome = catch_unwind(AssertUnwindSafe(|| match method {
        INITIALIZE => call(message, |p| handler.initialize(p)),
        INDEX => call(message, |p| handler.index(p)),
        SHUTDOWN => {
            *shutdown = true;
            Ok(Ok(serde_json::Value::Null))
        }
        other => Err((METHOD_NOT_FOUND, format!("unknown method {other}"))),
    }));
    let outcome = outcome.unwrap_or_else(|panic| {
        let text = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown".to_owned());
        Err((INTERNAL_ERROR, format!("panic: {text}")))
    });
    Box::new(move |id| match outcome {
        Ok(Ok(value)) => Message::response(id, value).expect("a value serializes"),
        Ok(Err(text)) => Message::failure(Some(id), INTERNAL_ERROR, text),
        Err((code, text)) => Message::failure(Some(id), code, text),
    })
}

type Outcome = Result<Result<serde_json::Value, String>, (i64, String)>;

fn call<P, R: Serialize>(message: Message, run: impl FnOnce(P) -> Result<R, String>) -> Outcome
where
    P: serde::de::DeserializeOwned,
{
    let params = message
        .into_params::<P>()
        .map_err(|e| (INVALID_PARAMS, e.to_string()))?;
    Ok(run(params).and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string())))
}
