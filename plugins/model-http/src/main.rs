//! `model-http`: models behind the standard HTTP APIs. A provider declares
//! which API it speaks (`spec`), where (`baseUrl`) and how it is
//! authenticated (`keyEnv`); every request names the model. The specs are
//! `openai` (Chat Completions, which OpenRouter, Ollama, vLLM and LM Studio
//! also speak) and `typesafe` (System One). Others join as further modules.

mod http;
mod openai;
mod serve;
mod typesafe;

use std::{
    io::{stdin, stdout},
    process::ExitCode,
};

use lighthouse_protocol::serve as protocol_serve;

fn main() -> ExitCode {
    let mut handler = serve::Plugin::new(Box::new(serve::Specs));
    match protocol_serve(stdin().lock(), stdout().lock(), &mut handler) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("model-http: {e}");
            ExitCode::FAILURE
        }
    }
}
