# fake-model

A deterministic, offline model for tests and examples: a real plugin by manifest, and a `command` provider.

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` only. Built by `lighthouse-test-support` for the tests; not part of `make plugins`.

## Entry points

- No arguments: a plugin speaking the protocol (`classify`, `train`, which keeps what it was given in the directory it is told).
- `command`: reads a System One body on stdin, prints the answer, and appends a mark to the file `calls` per run.
- `weak`: answers 0.5 in an envelope, as fenced text.
- `fail`: exits with an error.

## Invariants

- The answer follows the source of the subject: `maybe_bad` is surely a violation, `maybe_good` surely not, anything else unsure. Tests rely on this; changing it breaks the model, eval and train tests.
- It stays a plugin with a manifest, not a test-support shortcut, so tests go through discovery, the handshake and the same protocol as a real model.

## Tests

No tests of its own; used by `crates/lighthouse-cli/tests/it/models.rs` and `models_plugin.rs`; `cargo test -p lighthouse-cli`.
