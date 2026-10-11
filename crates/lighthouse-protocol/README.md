# lighthouse-protocol

The wire contract between the host and plugin processes: the language-provider protocol (`initialize`, `index`) and the model protocol (`lighthouse/classify`, `lighthouse/train`), as Content-Length frames over stdio.

**Layer.** L0; depends on no workspace crate. Used by plugin, spec, rpc and the plugins.

## Entry points
- `VERSION`, `INITIALIZE`, `INDEX`, `CLASSIFY`, `TRAIN`, `SHUTDOWN`, `EXIT`: version and method names, plus the capability constants.
- `read_message`, `write_message`, `Message`: the frames.
- `serve`, `Handler`: the loop a plugin process runs.
- `InitializeParams`/`InitializeResult`, `IndexParams`/`IndexResult`, `Fragment`: the language wire (`language/`).
- `SystemOneRequest`/`SystemOneResponse`, `Question`, `Answer`, `FineTuningJob`: the model API bodies (`model/`).
- `prompt`, `response_schema`: the System One prompt text and its response schema.

## Invariants
- Peers agree on `VERSION` exactly; a mismatch refuses the handshake.
- A change to the wire bumps `VERSION` and changes `plugins/lang-go/sdk` in the same commit; the conformance suite is the only guard on the Go twin.
- The language wire mirrors the code model and is converted in `rpc::convert`, the only place wire types become model types. The model API bodies are published bodies used verbatim, not mirrored.
- The protocol is specified in [docs/plugin-protocol.md](../../docs/plugin-protocol.md); this crate holds no I/O beyond framing.

## Tests
`crates/lighthouse-protocol/tests/it`; `cargo test -p lighthouse-protocol`.
