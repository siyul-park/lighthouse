# lang-toml

The TOML host-file provider: parses TOML files with `toml_edit` and returns each document as a JSON tree with the map from every value to its bytes, for the host to select source regions from. It has no code model of its own.

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` only, whose `serve_host` is the rest of the provider. Built by `make plugins` into `target/plugins/lang-toml`.

## Entry points

- `main.rs`: the language (`toml`, glob `**/*.toml`) and `serve_host`.
- `tree.rs`: tables, arrays of tables, inline tables and values as JSON, with a range for every node the parser locates.
- `strings.rs`: the host bytes of each decoded byte of a string: basic and literal, single-line and multi-line, with escapes.

## Invariants

- A datetime, and a float JSON cannot hold (`inf`, `nan`), is kept as a string node of its source text; the rest of the file still has its tree.
- A string that cannot be mapped to exact host bytes makes the whole file an `incomplete` entry rather than a tree with a wrong map.
- Capability `source-ast` only: the provider rejects selected regions as incomplete.

## Tests

`cargo test -p lang-toml`; the shared provider is tested in `crates/lighthouse-protocol/tests/it/host.rs`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
