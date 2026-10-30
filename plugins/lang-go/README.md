# lang-go

The Go language provider: loads packages with `go/packages`, type-checks with `go/types`, and speaks the plugin protocol on stdin and stdout.

**Layer.** Out-of-process plugin (a Go module, outside the cargo workspace); implements the protocol by hand in `sdk/` and imports no host crate. Built by `make plugins` into `target/plugins/lang-go`.

**Pack.** The plugin also provides the decision pack `go` (`decisions/go/`, named by `spec.catalog` in the manifest). The host reads it without starting the process, so listing `lang-go` in `plugins` makes `go/recommended` available to `extends`; `make plugins` copies `decisions/` next to the binary.

## Entry points

- `cmd/lang-go`: the binary, `sdk.Serve(provider.New(...))`.
- `provider/`: `Initialize` and `Index`; one file per concern (flow, uses, ids, interfaces, test cases, comments).
- `provider/cache*.go`: the fragment cache.
- `provider/region*.go`: the Go fences of host files (see below).
- `sdk/`: frames (`frame.go`), wire types (`wire.go`), the serve loop (`server.go`), result validation.

## Invariants

- `sdk` is the Go twin of `lighthouse-protocol`: `ProtocolVersion` equals protocol `VERSION`, and a change to either is a change to both in the same commit. The conformance goldens in `crates/lighthouse-rpc/tests/conformance/go` and the transcripts in `protocol/conformance`, which the sdk replays too, are the guard that they agree.
- Edges come from `go/types`, not from names; the fragment cache in `provider/cache*.go` is keyed by file text and package context.
- The tests of `sdk` and `provider` validate results against the protocol JSON Schema.

## Fences of host files

A Go fence of a Markdown file is a unit of its own, as an importer sees the module around the file: an overlay `<md dir>/.lighthouse-snippets/<md name>.<n>/snippet.go`, one package. Every unit of a module goes through the one `packages.Load` of that module, so N fences cost one load (`RunStats.Loads`).

- A fence with a `package` clause is a file as it is; declarations get `package main`; statements go in `func main`. The lines that wrap the fence are outside the host mapping, so a symbol, finding or fix that lands only on them is dropped, and what the wrapper declared or called is the unit's module's.
- Imports the fence uses and does not write are settled by name: a name the standard library or the module holds once, or the root package of a requirement in the module cache, at once (`known.go`); goimports (`golang.org/x/tools/imports`) is asked once for the module, for the names with several candidates or that only a requirement's sub-package may have. A name no one knows is left unresolved: prose defines names.
- `language.context.package = "<dir>"` overlays the fence into that package, under its name, so unexported names resolve; its symbols belong to the package's own module (the fence is code in the package).
- A fence that is not valid Go is a notice; one that does not type-check (names the prose defines) is counted in one notice and what resolves is reported. Neither is a gap. The names the type checker does not know are `syntactic` edges to the bare name (`calls Parse`, `references text`), which the model resolves when the project has one symbol so named.

## Tests

`cd plugins/lang-go && go test ./...` (part of `make test`). Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
