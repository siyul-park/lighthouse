# lang-rust

The Rust language provider: reads sources with `syn`, resolves names through its own module tree, and speaks the plugin protocol on stdin and stdout.

## Role

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` and its own libraries, never on a host crate. It implements `lighthouse_protocol::language::Provider` and is served by `language::serve`.

**Pack.** The plugin also provides the decision pack `rust` (`decisions/rust/`, named by `spec.catalog` in the manifest). The host reads it without starting the process, so listing `lang-rust` in `plugins` makes `rust/recommended` available to `extends`; `make plugins` copies `decisions/` next to the binary.

## Modules

- `main.rs`: the entry; `provider.rs`: the `Provider` (manifest, `index`) and the packages of a request.
- `index.rs` (with `index/types.rs`): the fragment of one file; `cache.rs`: the fragment cache.
- `project.rs`, `project/cargo.rs`: the module tree of every crate target and the packages from `Cargo.toml`.
- `names.rs`, `names/` (declare, resolve, query): what every module declares and imports, and path resolution; `source.rs`, `source/`: the syntax tree handed to the host (`source-ast`).
- `functions.rs`: one function body, with `functions/edges.rs` (calls and references), `functions/flow.rs`, `functions/events.rs` and `functions/signature.rs`.
- `extent.rs`, `visibility.rs`, `comments.rs`, `tests.rs`, `regions.rs` (with `regions/`): the roles named after what they fill; `syntax.rs` and `macros.rs` are the syn helpers they share.

## Facts and capabilities

| Fact | Source |
| --- | --- |
| `modules` | one per module of every crate target, named by its path from the crate root |
| `symbols` | functions, methods, types, interfaces (traits), fields, variants, constants, variables (statics), tests; `extent`, `doc`, `optional` and `type_ref` of fields |
| `edges` | `calls`, `references`, `imports`, `implements`, `contains` with sites; `syntactic` or `heuristic` where names are not settled |
| `functions` | flow, nesting, statements, tokens, signature, `events` (panicking macros, `unwrap`, `expect`), `forwards_to`, `implementation`, `constructs`, `manual_assertions` |
| `tests` | `#[test]` and its framework variants, in unit and integration targets |
| `comments`, `generated` | doc and line comments; `@generated` headers |

Capabilities: `extent`, `reference-sites`, `overlays`, `source-ast`.

## Project model

- Names are resolved syntactically and macros are never expanded, so edges are a lower bound. A method called through a receiver of unknown type is a `heuristic` edge to every non-`pub` method of that name, which counting rules ignore; resolving it differently makes rules report code with unseen callers.
- Cargo is never run: no build script, no network, no lock file needed.
- The cache keys a file by its text and a digest of what other files and manifests contribute; a body edit re-analyzes one file, any other change misses everything.
- `testdata` and hidden directories get empty fragments; a file that does not parse is an `incomplete` entry.
- A change to the wire goes through protocol `VERSION` and the other providers in the same commit.

### Fences of host files

A Rust fence of a Markdown file is a unit of its own, as rustdoc reads a doctest: a crate that uses the crate around the file from outside, so `use <crate>::` paths are edges to that crate's public symbols. The crate is the library of the nearest `Cargo.toml` with a `[package]`; without one only the standard library resolves.

- A fence that parses as items stays as it is; statements are the body of a `fn main`. Wrapper lines and `#` lines are analyzed and never placed: rustdoc's hidden lines (`##` shows a `#`), outside the host mapping, so what only they declare is dropped and what they call is the module's.
- `language.context.crate = "<module file>"` makes the fence code inside that module, where `crate::` and `super::` resolve; its symbols live in a module of their own.
- A fence that is not valid Rust is a notice; one that uses names the project does not hold is counted in one notice and its edges stay syntactic. Neither is a gap.
- `edition20xx` of the info string (`context.info`) is the `edition` fact of the host file when its fences agree; else the crate's edition around the file, else none, and fences that disagree are a notice. The facts of a region's file are merged over its host's; syn reads every edition the same.

## Conformance

Goldens: `crates/lighthouse-rpc/tests/conformance/rust`. Shared cases (the same name in every language plugin): `comments`, `errors`, `excluded`, `extents`, `flow`, `generated`, `signatures`, `tests`, `visibility`, and `events`. Own cases: `bins`, `calls`, `editions`, `guesses`, `impls`, `layout`, `macros`, `modules`, `private-traits`, `reexports`, `traits`, `support`, `unpublished`, `workspace`.

## Build

`make plugins` builds `target/plugins/lang-rust` (`cargo build --release -p lang-rust`).

## Tests

`plugins/lang-rust/tests/it`; `cargo test -p lang-rust`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
