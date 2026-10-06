# Plugin protocol 0.1

Normative contract between Lighthouse (the host) and a language plugin: a
separate process that analyzes source in its own language with native tools.
Rules, analyzers and presets stay in the host; a plugin only turns files into
the code model. The wire types are defined by
[`protocol/schema/lighthouse-protocol-0.1.json`](../protocol/schema/lighthouse-protocol-0.1.json),
generated from the `lighthouse-protocol` crate and checked in; where this text
and the schema disagree, the schema wins. The wire model is separate from the
host's internal model: internal changes are not protocol changes.

Key words MUST, SHOULD and MAY are used as in RFC 2119.

## Transport

- stdio of the plugin process; the host starts it with the project root as
  working directory, in its own process group (unix) so a timeout or crash
  takes grandchildren down too.
- JSON-RPC 2.0 messages, UTF-8, each framed as in LSP:
  `Content-Length: <bytes>\r\n\r\n<body>`. Other headers are ignored.
- stdout carries messages only; the host ignores notifications and unsolicited
  messages. stderr is free text; the host keeps the last 50 lines, cut to 2 KiB
  each, and shows them as notices with a count of dropped lines. Writes to the
  plugin never block the host past a request's deadline.
- Requests are answered in order, one at a time. A host never sends a request
  before the previous response arrived.

## Versioning

`protocolVersion` is `"0.1"`. Host and plugin MUST agree exactly; a plugin that
receives another version answers `initialize` with an error, a host that
receives another version refuses the plugin. Receivers MUST ignore unknown
fields. Fields of protocol messages are camelCase (`protocolVersion`,
`clientInfo`); fields of the code model keep the model's snake_case names
(`test_of`, `max_nesting`).

Reserved for later versions, not implemented in 0.1: `context.overlays`
(content of unsaved files; hosts do not send it), `clone_fingerprint`, and
further methods for rules and analyzers.

## Methods

### `initialize` (request)

Params `{ root, protocolVersion, clientInfo: { name, version } }`, `root`
absolute. Result:

```json
{ "id": "lang-go", "version": "0.1.0", "protocolVersion": "0.1",
  "languages": [{ "id": "go", "globs": ["**/*.go"], "priority": 0,
                  "conventions": { "test_globs": ["**/*_test.go"] },
                  "capabilities": ["semantic-edges"] }] }
```

- `id` MUST equal the `id` of the plugin's manifest.
- `globs` select project-relative files (`*` and `?` stop at `/`; a `**`
  component spans directories). Of several languages matching a file, the higher
  `priority` wins, then registration order; a `fallback: true` language only
  claims files no other language claims.
- `conventions.test_globs` mark test files for the whole host: rules skip or
  target them by this flag.
- `capabilities` list what the provider guarantees. `semantic-edges`: `calls`,
  `references`, `implements` and `accesses-private` edges are resolved by the
  language's own semantic analysis (types, scopes), not by name matching. A rule
  that requires a capability is skipped, with a notice, for languages without it.
  Hosts ignore capability names they do not know.

### `index` (request)

Params:

```json
{ "project": { "root": "/abs/root" }, "language": "go",
  "files": [{ "path": "pkg/a.go", "hash": "<sha256 hex>" }],
  "context": { "options": { "go": { "tags": ["integration"] } } } }
```

`files` are every project file of `language`, project-relative with `/`
separators, sorted. The request is project-level: one call carries the whole
batch and the provider decides which packages, modules or build units it must
load to analyze them, reading files from disk. Analysis scope is never
narrowed to the files a user asked to see; reporting is the host's concern.
`context.options` holds the `[languages.<id>]` tables of `lighthouse.toml`
for every language, as JSON; a provider reads its own entry and MUST reject
unknown keys in it (as an `incomplete` entry, see below).

Result `{ fragments, notices, incomplete }`:

- `fragments`: one per analyzed file, sorted by path, each
  `{ file: { path, generated? }, modules, symbols, edges, functions, tests }`.
  A requested file with no fragment is incomplete. Files the provider
  deliberately skips (excluded by the build context, `testdata`) get an empty
  fragment.
- `notices`: messages that leave the analysis complete (for example files
  excluded by build constraints).
- `incomplete`: `{ path?, reason }` for every file, or without `path` every
  scope, that could not be analyzed fully: load, parse and type errors, missing
  dependencies. A file listed here MAY still have a fragment with what could be
  recovered. Never drop a gap silently: "not checked" is not "passed".

### `shutdown` (request) and `exit` (notification)

`shutdown` is answered with `null`; the plugin then waits for `exit` and
terminates with code 0. A plugin whose input closes without `exit` terminates
with a non-zero code.

## Errors and failure handling

A response MAY carry a JSON-RPC `error`: `-32700` malformed input (the plugin
then stops), `-32601` unknown method, `-32602` invalid params, `-32603`
internal error. The host treats failures as follows, always turning them into
`incomplete` entries and never into a crash:

| Failure | Host behavior |
|---|---|
| `error` response to `index` | batch incomplete; plugin stays usable |
| crash, closed output, malformed or invalid message | batch incomplete; plugin is killed and every later call fails the same way |
| no answer within the request timeout (default 600 s, `timeout` of the plugin entry in seconds; the first index may build a cold cache) | as above; the whole process group is killed |
| fragment for a file that was not requested | ignored, with a notice |
| symbol id that violates the format below | message malformed |
| failure during `initialize`: crash, timeout, malformed or refused answer | the plugin is registered without languages and the run is incomplete (exit code 3) |
| other protocol version, other id than the entry names, binary that cannot start | configuration error, exit code 2 |
| duplicate fragment, or a fragment whose symbols or edges belong to another file | dropped with a notice; the file is incomplete |

## Code model

### Paths, modules, symbols

- Paths are project-relative with `/`. A **module** is a package, directory or
  namespace; its `path` identifies it project-wide. Go: the package directory
  relative to the root (`.` for the root), plus `[test]` for an external test
  package, whose module has `test_of` set to the module it tests.
- A **symbol id** is the single stable identity of a declaration:

  ```
  id   = module "::" { owner "::" } name "#" kind
  kind = function | method | type | field | const | var | interface | test
  ```

  `owner` names each enclosing declaration from the outside in (a method or
  field has its type, an interface method its interface). `module` contains no
  `::`. Two declarations never share an id; build variants of one declaration
  produce the same id and the host keeps the first. The `owner` field of a
  symbol is the id of its direct container.
- `visibility`: `public` (importable by anyone), `internal` (importable only
  inside the project), `private`.
- `span`: 1-based `line`, 1-based `col` counted in bytes, `end` exclusive.

### Edges

`{ kind, from, to, resolution }`. `from` is `{ "module": path }` or
`{ "symbol": id }`. `to` is a string: a module path, a full symbol id, or a
kind-less id (`module::owner::name`) which the host resolves when it names
exactly one symbol (for `calls`, exactly one function or method). Providers
emit `calls`, `references`, `implements` and `accesses-private` edges only for
targets inside the project; uses of the standard library and of dependencies
are left out. `imports` edges are kept for external packages, naming them by
import path, because the dependency itself is the fact.

| kind | meaning |
|---|---|
| `contains` | module or type contains a symbol |
| `imports` | module depends on another module |
| `calls` | function or method calls a function or method |
| `references` | use as a value: function value, field, type, constant, variable |
| `implements` | type satisfies an interface declared in the project |
| `accesses-private` | use of a member the language keeps private from outside the unit that owns it; the unit (type, module, package) is language-dependent, and a provider emits it only where such a use is possible |

`resolution` is `semantic` when the target comes from type information and
`syntactic` when it comes from names alone.

### Function summaries and control flow

`functions` holds one summary per function or method with a body:
`max_nesting` (deepest nesting level, a flat body is 0), `statements` (all
statements in the body, case clauses counting as statements), `top_level`
(statements directly in the body), `params`, `returns`, `tokens` (leaf tokens of
the body) and `forwards_to` (set when the body is one call that passes the
receiver and every parameter on, in order, naming the callee).

`flow` lists control-flow constructs in source order, each with the number of
enclosing nesting constructs (nested functions included) around it. Providers
map syntax onto these kinds:

| kind | maps |
|---|---|
| `if`, `else-if`, `else` | conditional, chained conditional, final branch; an `else if` is `else-if` at the nesting of its `if` |
| `switch` | `switch`, type switch, `match`, `select`: one event; `arms` counts arms other than the default; `returning` when every arm, default included, is a single return |
| `loop` | every loop form |
| `catch` | one per exception handler |
| `jump` | `goto`, labeled `break` and `continue` |
| `logic` | one event per run of like `&&`/`||` operators; `operators` counts them |
| `recursion` | a call of the enclosing function, resolved semantically |

Nested functions and closures emit nothing but raise the nesting of what they
contain. Cyclomatic complexity is `1 + if + else-if + loop + catch + switch
arms + logic operators`; cognitive complexity follows Campbell, SonarSource 2018.

### Tests

`tests` holds one entry per test case: `symbol` (a `test` symbol), `nesting`
(depth of nested sub-tests), `style` (`table` when the test loops over cases
written in the test, else `scenario`) and `targets` (edge targets the test calls
or references, in order of first use).

## Discovery

A plugin directory holds `lighthouse-plugin.toml` and the executable:

```toml
id = "lang-go"
version = "0.1.0"
command = "./lang-go"   # relative path: next to the manifest; bare name: PATH
args = []
```

Only plugins listed in `lighthouse.toml` are started. Manifests that do not parse are ignored (with a notice) unless the directory
is named after a listed id. A bare id is looked up in
`./.lighthouse/plugins/*/`, `~/.lighthouse/plugins/*/` and `<executable
dir>/plugins/*/`; an entry `{ id = "lang-go", path = "plugins/lang-go",
timeout = 120 }` names the directory itself (relative to the config) and is how
a development checkout is used. The same id in more than one place, an in-process
plugin included, is an error: there is no shadowing.

## Trust model

Running `lighthouse check` executes the plugins that the project's
`lighthouse.toml` names, with the user's privileges, exactly as running a
project's build scripts does. There is no trust prompt or flag: check only
projects whose configuration you trust. Plugins found by search are started only
when listed; an unlisted manifest is never executed.

## Conformance

Every provider passes the golden suite in `plugins/conformance/<language>`: each
case is a project, option variants (`options[.variant].json`) and the exact
`index` result expected for them (`expected[.variant].json`), covering symbols,
edges, visibility, ownership, spans, flow, tests, capabilities and a
deterministic order. `cargo test -p lighthouse-rpc` runs the suite against the
built plugin and validates every result against the schema.

## `lang-go`

Loads packages with `golang.org/x/tools/go/packages` (tests included) and
type-checks with `go/types`.

- Context: `[languages.go]` accepts `tags` (build tags), `env` (for example
  `GOOS`, `GOARCH`, `GOTOOLCHAIN`), `go` and `small_interfaces`.
  - `go` is the go binary: an absolute path or a name looked up on `PATH`. The
    default is the `go` found on `PATH`, which is the project's own toolchain
    when a version manager shim such as goenv is installed. go/packages only
    finds `go` on the plugin's own `PATH`, so an explicit `go` is put first on
    it for the duration of a load.
  - `GOTOOLCHAIN` defaults to `local` so analysis never downloads a toolchain.
  - `small_interfaces` also emits `implements` edges for one-method
    interfaces, which are skipped by default because nearly every type
    satisfies them by accident.
  - Files excluded by the build context are reported once as an intentional
    notice, not as incomplete: one analysis has one build context.
- Only targets inside the project become `calls`, `references` and `implements`
  edges. No `accesses-private` edges: Go cannot reach an unexported member from
  another package.
- Packages are loaded per `go.mod`; Go files with no `go.mod` form one synthetic
  module. `testdata`, `vendor` and names starting with `.` or `_` are skipped as
  the go tool skips them.
- `package main` makes every symbol `private`; exported symbols under an
  `internal` directory are `internal`.
- Load, parse and type errors become `incomplete` entries per file, with
  `line:col: message`.
