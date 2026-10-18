//! A client for tests: records what it is asked and answers with the next reply.

use std::time::Duration;

use lighthouse_protocol::ErrorObject;
use serde_json::Value;

use crate::http::Client;

/// Records what it is asked and answers with the next reply.
pub struct Recording {
    replies: std::sync::Mutex<Vec<Value>>,
    asked: std::sync::Mutex<Vec<(String, Value)>>,
}

impl Recording {
    pub fn replying(replies: Vec<Value>) -> Self {
        Self {
            replies: std::sync::Mutex::new(replies.into_iter().rev().collect()),
            asked: std::sync::Mutex::default(),
        }
    }

    /// The urls and bodies posted so far.
    pub fn asked(&self) -> Vec<(String, Value)> {
        self.asked.lock().unwrap().clone()
    }
}

impl Client for Recording {
    fn post(
        &self,
        url: &str,
        _: &[(String, String)],
        body: &Value,
        _: Duration,
    ) -> Result<Value, ErrorObject> {
        self.asked
            .lock()
            .unwrap()
            .push((url.to_owned(), body.clone()));
        self.replies
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| "no reply".into())
    }
}
