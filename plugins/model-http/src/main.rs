//! `model-http`: models behind the standard HTTP APIs. A provider declares
//! which APIs it speaks (`spec`), where (`baseUrl`) and how it is
//! authenticated (`keyEnv`); every request names the model. Each published API
//! is one module of free functions, one per operation used: `openai`
//! (Chat Completions, which OpenRouter, Ollama, vLLM and LM Studio also
//! speak) and `typesafe` (System One). `chat` adapts one to the other.

mod chat;
mod endpoint;
mod http;
mod openai;
#[cfg(test)]
mod recording;
mod serve;
mod typesafe;

use std::{
    io::{stdin, stdout},
    process::ExitCode,
};

use lighthouse_protocol::serve as protocol_serve;

fn main() -> ExitCode {
    let mut handler = serve::Plugin::new(Box::new(http::Http::new()));
    match protocol_serve(stdin().lock(), stdout().lock(), &mut handler) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("model-http: {e}");
            ExitCode::FAILURE
        }
    }
}
