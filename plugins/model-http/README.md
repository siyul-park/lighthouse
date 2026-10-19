# model-http

The model provider over the standard HTTP APIs: TypeSafe System One and the OpenAI Chat Completions family (OpenRouter, Ollama, vLLM, LM Studio).

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` and `async-openai` types only. Built by `make plugins`; its manifest provides the model `model-http`.

## Entry points
- `openai`, `typesafe`: one module per published protocol, one free function per operation used.
- `chat`: adapts a System One request to Chat Completions (a prompt asking for `true` or `false`, P(fail) read from the logprobs of the answer token). It is the only use of Chat Completions: the logprobs adapter behind `classification`, never a generator.
- `endpoint`: the provider `options`: `protocol`, `url`, `headers`, `body`, `timeout`.
- `http`: the one I/O seam, `Client`, over one shared agent; `serve`: the handshake and handler, which picks the protocol a provider declares for `classify` (`typesafe` before `openai`).

## Invariants
- Request and response bodies are the published ones (protocol's `SystemOneRequest`/`SystemOneResponse`, `async-openai` types), used verbatim; there is no home-made API.
- A key is a `${NAME}` in a header value, read from the environment when a request is made, never from configuration.
- A model that returns no logprobs cannot classify: the request fails saying so; a number the model writes is never read as a probability.
- Only a busy (429, 503) or overloaded (529) service is asked again, after its `Retry-After`; nothing else is retried.

## Tests
Unit tests in `src/` with `recording.rs` as the client; `cargo test -p model-http`. Models in [docs/models.md](../../docs/models.md).
