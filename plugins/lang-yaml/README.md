# lang-yaml

The YAML host-file provider: parses YAML files with `saphyr` and returns each document as a JSON tree with the map from every value to its bytes, for the host to select source regions from (for example the CEL expressions of a decision). It has no code model of its own.

## Role

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` only. It implements `HostParser`, and `HostProvider::new(parser)` is the `Provider` that `language::serve` serves. Built by `make plugins` into `target/plugins/lang-yaml`.

## Modules

- `main.rs`: the entry, `language::serve(HostProvider::new(Yaml))`.
- `provider.rs`: the parser: the documents as `{"kind": "Stream", "documents": [...]}`, with a range for every node; and the language (`yaml`, globs `**/*.yaml`, `**/*.yml`).
- `regions.rs`: the text of a string scalar and the host bytes of each decoded byte: plain, quoted with escapes and folding, and block (`|`, `>`).
- No `symbols` and no `comments`: the parser drops comments.

## Facts and capabilities

The fragment of a host file is the file alone (so file decisions see it) and its comments where the parser keeps them; the tree is the `Source`.

| Fact | Source |
| --- | --- |
| `source` | the stream tree and its map, for files asked with `source_ast` |
| `comments` | none: the parser drops them |

Capabilities: `source-ast`, `overlays`.

## Project model

- Plain scalars resolve as the YAML 1.2 core schema, as `saphyr` resolves them; a float JSON cannot hold (`.inf`, `.nan`) stays its text.
- A string scalar that cannot be mapped to exact host bytes makes the whole file an `incomplete` entry rather than a tree with a wrong map.
- Selected regions (a fence or scalar whose language is YAML) are read as documents; one that is not is a notice, one the tree cannot expose is an `incomplete` entry.

## Conformance

There are no golden cases: the shared provider is tested in `crates/lighthouse-protocol/tests/it/host.rs` and the tree and its map in this plugin's tests.

## Build

`make plugins` builds `target/plugins/lang-yaml` (`cargo build --release -p lang-yaml`).

## Tests

`cargo test -p lang-yaml`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
