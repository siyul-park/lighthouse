use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

const JSONRPC: &str = "2.0";
const MAX_BODY: usize = 256 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("malformed header: {0}")]
    Header(String),
    #[error("message of {0} bytes exceeds the {MAX_BODY} byte limit")]
    TooLarge(usize),
    #[error("malformed message: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// One JSON-RPC 2.0 message: a request or notification (`method`), or a
/// response (`result` or `error`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl Message {
    fn empty() -> Self {
        Self {
            jsonrpc: JSONRPC.to_owned(),
            id: None,
            method: None,
            params: None,
            result: None,
            error: None,
        }
    }

    pub fn request(
        id: i64,
        method: &str,
        params: impl Serialize,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id: Some(Id::Number(id)),
            method: Some(method.to_owned()),
            params: Some(serde_json::to_value(params)?),
            ..Self::empty()
        })
    }

    pub fn notification(method: &str) -> Self {
        Self {
            method: Some(method.to_owned()),
            ..Self::empty()
        }
    }

    pub fn response(id: Id, result: impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id: Some(id),
            result: Some(serde_json::to_value(result)?),
            ..Self::empty()
        })
    }

    pub fn failure(id: Option<Id>, code: i64, message: impl Into<String>) -> Self {
        Self {
            id,
            error: Some(ErrorObject {
                code,
                message: message.into(),
                data: None,
            }),
            ..Self::empty()
        }
    }

    /// Deserializes `result` of a response; an `error` response is returned
    /// as the `Err` side.
    pub fn into_result<T: DeserializeOwned>(
        self,
    ) -> Result<Result<T, ErrorObject>, serde_json::Error> {
        if let Some(error) = self.error {
            return Ok(Err(error));
        }
        serde_json::from_value(self.result.unwrap_or(Value::Null)).map(Ok)
    }

    pub fn into_params<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.params.unwrap_or(Value::Null))
    }
}

/// Reads one `Content-Length` framed message; `None` at a clean end of input.
pub fn read_message(reader: &mut impl BufRead) -> Result<Option<Message>, FrameError> {
    let mut length = None;
    let mut first = true;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return if first {
                Ok(None)
            } else {
                Err(FrameError::Header("input ended inside a header".to_owned()))
            };
        }
        first = false;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(FrameError::Header(line.to_owned()));
        };
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| FrameError::Header(line.to_owned()))?,
            );
        }
    }
    let length = length.ok_or_else(|| FrameError::Header("missing Content-Length".to_owned()))?;
    if length > MAX_BODY {
        return Err(FrameError::TooLarge(length));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

pub fn write_message(writer: &mut impl Write, message: &Message) -> Result<(), FrameError> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}
