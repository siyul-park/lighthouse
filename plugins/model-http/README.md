# model-http

The model provider over the standard HTTP APIs: TypeSafe System One and the OpenAI Chat Completions family (OpenRouter, Ollama, vLLM, LM Studio).

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` and `async-openai` types only. Built by `make plugins`; its manifest provides the model `model-http`.

## Entry points
- `openai`, `typesafe`: one module per published API, one free function per operation used.
- `chat`: adapts a System One request to Chat Completions (a prompt asking for the probability of yes, `json_schema` response format).
- `dispatch`: the table of which function serves which protocol method.
- `endpoint`: the provider `options`: `spec`, `baseUrl`, `keyEnv`.
- `http`: the one I/O seam, `Client`; `serve`: the handshake and handler.

## Invariants
- Request and response bodies are the published ones (protocol's `SystemOneRequest`/`SystemOneResponse`, `async-openai` types), used verbatim; there is no home-made API.
- The key is read from the environment variable `keyEnv` names, never from configuration.
- A provider without structured outputs fails the request with the error body it answered; there is no second request without it.
- Only a busy (429, 503) or overloaded (529) service is asked again, after its `Retry-After`; nothing else is retried.

## Tests
Unit tests in `src/` with `recording.rs` as the client; `cargo test -p model-http`. Models in [docs/models.md](../../docs/models.md).
