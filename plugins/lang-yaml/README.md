# lang-yaml

The YAML host-file provider: parses YAML files with `saphyr` and returns each document as a JSON tree with the map from every value to its bytes, for the host to select source regions from (for example the CEL expressions of a decision). It has no code model of its own.

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` only, whose `serve_host` is the rest of the provider. Built by `make plugins` into `target/plugins/lang-yaml`.

## Entry points

- `main.rs`: the language (`yaml`, globs `**/*.yaml`, `**/*.yml`) and `serve_host`.
- `tree.rs`: the documents as `{"kind": "Stream", "documents": [...]}`, with a range for every node.
- `scalar.rs`: the text of a string scalar and the host bytes of each decoded byte: plain, quoted with escapes and folding, and block (`|`, `>`).

## Invariants

- Plain scalars resolve as the YAML 1.2 core schema, as `saphyr` resolves them; a float JSON cannot hold (`.inf`, `.nan`) stays its text.
- A string scalar that cannot be mapped to exact host bytes makes the whole file an `incomplete` entry rather than a tree with a wrong map.
- Capability `source-ast` only: the provider rejects selected regions as incomplete.

## Tests

`cargo test -p lang-yaml`; the shared provider is tested in `crates/lighthouse-protocol/tests/it/host.rs`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
