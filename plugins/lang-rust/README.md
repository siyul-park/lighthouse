# lang-rust

The Rust language provider: reads sources with `syn`, resolves names through its own module tree, and speaks the plugin protocol on stdin and stdout.

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` and its own libraries, never on a host crate. Built by `make plugins` into `target/plugins/lang-rust`.

## Entry points
- `provider.rs`: `initialize` and `index` (capabilities `extent`, `reference-sites`, `overlays`; globs `**/*.rs`).
- `tree`, `cargo`: packages and crate targets from `Cargo.toml`, the module tree.
- `names/` (declare, resolve, export, query): what every module declares and imports, and path resolution.
- `body/` (flow, calls): calls, references and normalized control flow of a function.
- `extract.rs`: the fragment of one file; `cache.rs`: the fragment cache.

## Invariants
- Names are resolved syntactically and macros are never expanded, so edges are a lower bound. A method called through a receiver of unknown type is a `heuristic` edge to every non-`pub` method of that name, which counting rules ignore; resolving it differently makes rules report code with unseen callers.
- Cargo is never run: no build script, no network, no lock file needed.
- The cache keys a file by its text and a digest of what other files and manifests contribute; a body edit re-analyzes one file, any other change misses everything.
- A change to the wire goes through protocol `VERSION` and lang-go in the same commit.

## Tests
`plugins/lang-rust/tests/it`; `cargo test -p lang-rust`. Goldens: `crates/lighthouse-rpc/tests/conformance/rust`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
