# lang-rust

The Rust language provider: reads sources with `syn`, resolves names through its own module tree, and speaks the plugin protocol on stdin and stdout.

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` and its own libraries, never on a host crate. Built by `make plugins` into `target/plugins/lang-rust`.

**Pack.** The plugin also provides the decision pack `rust` (`decisions/rust/`, named by `spec.catalog` in the manifest). The host reads it without starting the process, so listing `lang-rust` in `plugins` makes `rust/recommended` available to `extends`; `make plugins` copies `decisions/` next to the binary.

## Entry points

- `provider.rs`: `initialize` and `index` (capabilities `extent`, `reference-sites`, `overlays`; globs `**/*.rs`).
- `tree`, `cargo`: packages and crate targets from `Cargo.toml`, the module tree.
- `names/` (declare, resolve, export, query): what every module declares and imports, and path resolution.
- `body/` (flow, calls): calls, references and normalized control flow of a function.
- `index.rs`: the fragment of one file; `cache.rs`: the fragment cache.
- `region.rs`, `region/`: the Rust fences of host files (see below).

## Invariants

- Names are resolved syntactically and macros are never expanded, so edges are a lower bound. A method called through a receiver of unknown type is a `heuristic` edge to every non-`pub` method of that name, which counting rules ignore; resolving it differently makes rules report code with unseen callers.
- Cargo is never run: no build script, no network, no lock file needed.
- The cache keys a file by its text and a digest of what other files and manifests contribute; a body edit re-analyzes one file, any other change misses everything.
- A change to the wire goes through protocol `VERSION` and lang-go in the same commit.

## Fences of host files

A Rust fence of a Markdown file is a unit of its own, as rustdoc reads a doctest: a crate that uses the crate around the file from outside, so `use <crate>::` paths are edges to that crate's public symbols. The crate is the library of the nearest `Cargo.toml` with a `[package]`; without one only the standard library resolves.

- A fence that parses as items stays as it is; statements are the body of a `fn main`. Wrapper lines and `#` lines are analyzed and never placed: rustdoc's hidden lines (`##` shows a `#`), outside the host mapping, so what only they declare is dropped and what they call is the module's.
- `language.context.crate = "<module file>"` makes the fence code inside that module, where `crate::` and `super::` resolve; its symbols live in a module of their own.
- A fence that is not valid Rust is a notice; one that uses names the project does not hold is counted in one notice and its edges stay syntactic. Neither is a gap.
- `edition20xx` of the info string (`context.info`) is the `edition` fact of the host file when its fences agree; else the crate's edition around the file, else none, and fences that disagree are a notice. The facts of a region's file are merged over its host's; syn reads every edition the same.

## Tests

`plugins/lang-rust/tests/it`; `cargo test -p lang-rust`. Goldens: `crates/lighthouse-rpc/tests/conformance/rust`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
