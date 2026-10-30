# lang-toml

The TOML host-file provider: parses TOML files with `toml_edit` and returns each document as a JSON tree with the map from every value to its bytes, for the host to select source regions from. It has no code model of its own.

## Role

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` only. It implements `HostParser`, and `HostProvider::new(parser)` is the `Provider` that `language::serve` serves. Built by `make plugins` into `target/plugins/lang-toml`.

## Modules

- `main.rs`: the entry, `language::serve(HostProvider::new(Toml))`.
- `provider.rs`: the parser: tables, arrays of tables, inline tables and values as JSON, with a range for every node the parser locates; and the language (`toml`, glob `**/*.toml`).
- `comments.rs`: the comments `toml_edit` keeps in the decorations of keys and values, grouped by adjacent lines.
- `regions.rs`: the host bytes of each decoded byte of a string: basic and literal, single-line and multi-line, with escapes.

## Facts and capabilities

The fragment of a host file is the file alone (so file decisions see it) and its comments where the parser keeps them; the tree is the `Source`.

| Fact | Source |
| --- | --- |
| `source` | the table tree and its map, for files asked with `source_ast` |
| `comments` | the comments of keys, values and tables |

Capabilities: `source-ast`, `overlays`.

## Project model

- A datetime, and a float JSON cannot hold (`inf`, `nan`), is kept as a string node of its source text; the rest of the file still has its tree.
- A string that cannot be mapped to exact host bytes makes the whole file an `incomplete` entry rather than a tree with a wrong map.
- Selected regions (a fence or scalar whose language is TOML) are read as documents; one that is not is a notice, one the tree cannot expose is an `incomplete` entry.

## Conformance

There are no golden cases: the shared provider is tested in `crates/lighthouse-protocol/tests/it/host.rs` and the tree and its map in this plugin's tests.

## Build

`make plugins` builds `target/plugins/lang-toml` (`cargo build --release -p lang-toml`).

## Tests

`cargo test -p lang-toml`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
