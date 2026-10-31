# lang-go

The Go language provider: loads packages with `go/packages`, type-checks with `go/types`, and speaks the plugin protocol on stdin and stdout.

## Role

**Layer.** Out-of-process plugin (a Go module, outside the cargo workspace); imports no host crate. It implements `sdk.Provider` and is served by `sdk.Serve`. Built by `make plugins` into `target/plugins/lang-go`.

**Pack.** The plugin also provides the decision pack `go` (`decisions/go/`, named by `spec.catalog` in the manifest). The host reads it without starting the process, so listing `lang-go` in `plugins` makes `go/recommended` available to `extends`; `make plugins` copies `decisions/` next to the binary.

## Modules

- `cmd/lang-go`: the entry, `sdk.Serve(os.Stdin, os.Stdout, provider.New(...))`.
- `sdk/`: the part no language shapes: frames (`frame.go`), the serve loop and `Provider` (`server.go`), result encoding (`result.go`), protocol constants (`protocol.go`) and the wire types (`wire.go`, generated).
- `provider/provider.go`: the `Provider` (manifest, `Index`); `index.go`, `index_unit.go`: one request and one unit to a `Fragment`.
- `project_*.go`: the resolver and the options; `cache_*.go`: the fragment cache.
- Roles, named after the `Fragment` field they fill: `symbols*.go`, `edges_*.go`, `functions_*.go` (events, forwarding), `flow.go`, `tests*.go`, `comments.go`, `regions*.go`; and `extent.go`, `generated.go`.

## Facts and capabilities

| Fact | Source |
| --- | --- |
| `modules` | one per package, with `test_of` for external test packages |
| `symbols` | functions, methods, types, struct fields, interface methods, constants, variables, tests; `extent`, `doc`, `optional`, `type_ref` of fields |
| `edges` | `calls`, `references`, `imports`, `implements`, `accesses-private` from `go/types`, with sites; `syntactic` for names the prose of a fence defines |
| `functions` | flow, nesting, statements, tokens, signature, `events` (`panic`, error comparisons and assertions, `fmt.Errorf` without `%w`), `forwards_to`, `implementation`, `constructs`, `configures`, `manual_assertions` |
| `tests` | `Test*` functions and their `t.Run` subtests, with table or scenario style |
| `comments`, `generated` | comment groups; the `Code generated ... DO NOT EDIT.` marker |

Capabilities: `semantic-edges`, `extent`, `reference-sites`, `overlays`, `source-ast`.

## Project model

- Edges come from `go/types`, not from names; the fragment cache in `provider/cache_*.go` is keyed by file text and package context.
- `sdk` is the Go twin of `lighthouse-protocol`: `ProtocolVersion` equals protocol `VERSION`, and a change to either is a change to both in the same commit. The wire types in `sdk/wire.go` are generated from `protocol/schema` by `sdk/gen` (`go generate ./sdk`); `make lint` fails when the file is stale.
- The tests of `sdk` and `provider` validate results against the protocol JSON Schema; the transcripts in `protocol/conformance` are replayed against the serve loop.

### Fences of host files

A Go fence of a Markdown file is a unit of its own, as an importer sees the module around the file: an overlay `<md dir>/.lighthouse-snippets/<md name>.<n>/snippet.go`, one package. Every unit of a module goes through the one `packages.Load` of that module, so N fences cost one load (`RunStats.Loads`).

- A fence with a `package` clause is a file as it is; declarations get `package main`; statements go in `func main`. The lines that wrap the fence are outside the host mapping, so a symbol, finding or fix that lands only on them is dropped, and what the wrapper declared or called is the unit's module's.
- Imports the fence uses and does not write are settled by name: a name the standard library or the module holds once, or the root package of a requirement in the module cache, at once (`regions_known.go`); goimports (`golang.org/x/tools/imports`) is asked once for the module, for the names with several candidates or that only a requirement's sub-package may have. A name no one knows is left unresolved: prose defines names.
- `language.context.package = "<dir>"` overlays the fence into that package, under its name, so unexported names resolve; its symbols belong to the package's own module (the fence is code in the package).
- A fence that is not valid Go is a notice; one that does not type-check (names the prose defines) is counted in one notice and what resolves is reported. Neither is a gap. The names the type checker does not know are `syntactic` edges to the bare name (`calls Parse`, `references text`), which the model resolves when the project has one symbol so named.

## Conformance

Goldens: `crates/lighthouse-rpc/tests/conformance/go`. Shared cases (the same name in every language plugin): `comments`, `errors`, `excluded`, `extents`, `flow`, `generated`, `signatures`, `tests`, `visibility`, and `events`. Own cases: `aliases`, `embedded`, `generics`, `layout`, `modules`, `nomod`, `roles`, `tags`, `wider`, `xtest`.

## Build

`make plugins` builds `target/plugins/lang-go`. `go generate ./sdk` rewrites `sdk/wire.go`.

## Tests

`cd plugins/lang-go && go test ./...` (part of `make test`). Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
