# lighthouse-rpc

Providers that run as processes: discovers and starts plugin processes, adapts them to `Plugin` and `ModelClient` over the protocol, and runs a model that is any program.

**Layer.** L2; depends on model, plugin, process, protocol, resource and spec.

## Entry points

- `register`: starts every plugin `lighthouse.toml` lists and puts it in the `Registry`.
- `search_dirs`, `discover`, `load`, `parse`: finding and reading `lighthouse-plugin.toml`.
- `RpcPlugin`: `connect` runs the handshake; `languages()` and the models it offers.
- `Command`: a `ModelClient` over a program that reads System One on stdin (classification), or a brief as plain text (edit).
- `Provides`, `Runtime`, `PluginSpec`: the manifest kind; `descriptors()` gives its schema.

## Invariants

- Only listed plugins start; manifests of unlisted plugins are never run and, when broken, ignored.
- An id found in two places, or also built in, is an error and nothing starts.
- A process that crashes, times out or answers wrongly during `initialize` is `incomplete`, not a configuration error; later calls fail fast.
- `convert.rs` is the one place wire types become model types; the protocol `VERSION` and identity must match exactly.
- A `command` model runs a program, so only a trusted project builds it.

## Tests

`crates/lighthouse-rpc/tests/it`; `cargo test -p lighthouse-rpc` (builds the plugins it runs; without a Go toolchain the Go cases skip unless `CI` is set). Golden fixtures for both language plugins are in `tests/conformance`; regenerate with `UPDATE_GOLDEN=1 cargo test -p lighthouse-rpc conformance`.
