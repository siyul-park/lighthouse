# lang-go

The Go language provider: loads packages with `go/packages`, type-checks with `go/types`, and speaks the plugin protocol on stdin and stdout.

**Layer.** Out-of-process plugin (a Go module, outside the cargo workspace); implements the protocol by hand in `sdk/` and imports no host crate. Built by `make plugins` into `target/plugins/lang-go`.

## Entry points
- `cmd/lang-go`: the binary, `sdk.Serve(provider.New(...))`.
- `provider/`: `Initialize` and `Index`; one file per concern (flow, uses, ids, interfaces, test cases, comments).
- `provider/cache*.go`: the fragment cache.
- `sdk/`: frames (`frame.go`), wire types (`wire.go`), the serve loop (`server.go`), result validation.

## Invariants
- `sdk` is the Go twin of `lighthouse-protocol`: `ProtocolVersion` equals protocol `VERSION`, and a change to either is a change to both in the same commit. The conformance goldens in `crates/lighthouse-rpc/tests/conformance/go` and the transcripts in `protocol/conformance`, which the sdk replays too, are the guard that they agree.
- Edges come from `go/types`, not from names; the fragment cache in `provider/cache*.go` is keyed by file text and package context.
- The tests of `sdk` and `provider` validate results against the protocol JSON Schema.

## Tests
`cd plugins/lang-go && go test ./...` (part of `make test`). Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
