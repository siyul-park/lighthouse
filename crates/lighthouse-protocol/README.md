# lighthouse-protocol

Wire types, framing and schemas for plugins and models. Layer L0. Never: any lighthouse crate.

The wire contract between the host and plugin processes: the language-provider protocol (`initialize`, `index`) and the model protocol (`lighthouse/classify`, `lighthouse/train`), as Content-Length frames over stdio.

**Depends on:** no workspace crate.

## Entry points

- `VERSION`, `INITIALIZE`, `INDEX`, `CLASSIFY`, `TRAIN`, `SHUTDOWN`, `EXIT`: version and method names, plus the capability constants.
- `ROUTES`: every method with its HTTP twin; model-http posts to its paths and docs/plugin-protocol.md lists the same table.
- `read_message`, `write_message`, `Message`: the frames.
- `serve`, `Handler`: the loop a plugin process runs.
- `InitializeParams`/`InitializeResult`, `IndexParams`/`IndexResult`, `Fragment`: the language wire (`language/`).
- `SystemOneRequest`/`SystemOneResponse`, `Question`, `Answer`, `FineTuningJob`: the model API bodies (`model/`).
- `prompt`: the System One prompt text, and the reading of an answer a program printed.
- `hash`, `canonical`, `stats`: the SHA-256, the canonical JSON it digests and the cross-validation folds that host and plugin must compute byte-identically (content addresses of datasets and artifacts, fold assignment); the host's other hashes use the same one.

## Invariants

- Peers agree on `VERSION` exactly; a mismatch refuses the handshake.
- A change to the wire bumps `VERSION` and changes `plugins/lang-go/sdk` in the same commit; the conformance suite is the only guard on the Go twin.
- The language wire mirrors the code model and is converted in `rpc::convert`, the only place wire types become model types. The model API bodies are published bodies used verbatim, not mirrored.
- The protocol is specified in [docs/plugin-protocol.md](../../docs/plugin-protocol.md); this crate holds no I/O beyond framing.

## Tests

`crates/lighthouse-protocol/tests/it`; `cargo test -p lighthouse-protocol`. The recorded exchanges in `protocol/conformance` are replayed by every plugin (`lighthouse-rpc`'s `transcript` test) and by the Go sdk.
