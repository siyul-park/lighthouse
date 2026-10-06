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
  Hosts ignore capability names they do not know. A provider without
  `semantic-edges` is syntactic: its edges come from names (and, where marked,
  guesses), which makes its caller and reference sets lower bounds; no separate
  capability announces that.

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
  `{ file: { path, generated? }, modules, symbols, edges, functions, tests, comments }`.
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
terminates with code 0. A plugin whose input closes before `shutdown` was
answered terminates with a non-zero code; input that closes after `shutdown` (a
host that went away) is a clean stop for providers built on the Rust server loop,
and tolerated by hosts.

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
- `role` (optional): on the declarations of a test file whose purpose the
  language can tell, `test-helper` (test code that checks on behalf of tests;
  in Go a function that takes `*testing.T`, `*testing.B`, `*testing.F` or
  `testing.TB`) or `fixture` (test data and the code that builds it: types,
  constants, variables and functions that need no handle). Test entry points
  are `test` symbols and carry no role; methods and fields carry none either.
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

`resolution` is `semantic` when the target comes from type information,
`syntactic` when it comes from names alone, and `heuristic` for a guess: the
edge says the source *may* use the target. The host ignores heuristic `calls`
edges when it counts callers and callees (fan-in, fan-out, caller lists) but
keeps heuristic `references`, so a rule asking "might this be used" fails safe.

### Function summaries and control flow

`functions` holds one summary per function or method with a body:
`max_nesting` (deepest nesting level, a flat body is 0), `statements` (all
statements in the body, case clauses counting as statements), `top_level`
(statements directly in the body), `params`, `returns`, `tokens` (leaf tokens of
the body) and `forwards_to` (set when the body is one call that passes the
receiver and every parameter on, in order, naming the callee).

`param_types` (optional) lists the project types the parameters name, receiver
excluded, as kind-less symbol ids (`module::Type`), so rules can ask whether a
function takes a value of some type.

`manual_assertions` (optional, test files only) counts the checks a function
writes out by hand: an `if` with no `else` whose condition compares or negates
and whose only effect is to fail the test (in Go, through the `testing`
handle). It lets rules ask whether tests use the project's assertion library.

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

### Comments

`comments` holds the comments of the file in source order, which lets rules judge
text no symbol carries (section banners, commented-out code). Adjacent line
comments with nothing else between them form one comment. Each entry is `{ span,
text, attached_to? }`: `text` is the source text with its comment markers, the
lines of one comment joined by `\n`; `attached_to` is the id of the symbol the
comment documents, set when the comment stands alone on the lines directly above
that symbol's declaration (attributes may sit between). Doc comments appear here
too, as written. The field was added to 0.1 before 1.0 and may be omitted by a
plugin that does not report comments; hosts then see none.

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

## `lang-rust`

Reads sources with `syn` and `proc-macro2`; it resolves names itself and never
runs cargo, a build script or the compiler, so analysis needs no network, no lock
file and no toolchain. Its capability list is empty: every edge has
`resolution: "syntactic"`, and rules that require `semantic-edges` are skipped for
Rust with a notice.

- Context: `[languages.rust]` accepts `unpublished` (`"auto"`, the default,
  `"public"` or `"internal"`); unknown keys are an `incomplete` entry.
  `unpublished` decides what the `pub` items of a library with `publish = false`
  are: with `auto` they stay `public` when another package of the project
  depends on the library (a contract between packages) and are `internal`
  otherwise.
- Packages and targets come from `Cargo.toml` (read with a TOML parser): the
  nearest manifest with a `[package]` above a file governs it. Targets are the
  library (`src/lib.rs` or `[lib] path`), binaries (`src/main.rs`, `src/bin/*`,
  `[[bin]]`), integration tests (`tests/*.rs`, `tests/*/main.rs`), examples,
  benches and the build script. Files no manifest governs form one synthetic
  package named `workspace` with the standard layout under the project root.
  Dependencies on other packages of the project, including renamed ones
  (`package = "..."`) and workspace members, resolve through the extern prelude;
  `publish = false` (also inherited from `[workspace.package]`) marks a package
  without outside consumers.
- Modules follow `mod` declarations: `name.rs`, `name/mod.rs`, `#[path]`, nested
  inline modules, with the directory rules of mod-rs and non-mod-rs files. Every
  `cfg` branch is followed. A `mod` with no file, or a file that does not parse,
  is an `incomplete` entry (with `line:col` for syntax errors); the file still has
  a fragment. A file that no crate target reaches is a notice, or an
  `incomplete` entry when another file of the project failed to parse, because
  the tree may then be cut short. A file shared by several targets (`mod common;`
  in each file of `tests/`) is analyzed once, as part of the first target, and
  named by a notice; the other targets resolve the module to it. `testdata` and
  hidden directories are skipped with empty fragments.
- Module paths use `/`, as the id format forbids `::` in a module: the library is
  the package name (`lighthouse_model`), its modules `lighthouse_model/ucm`;
  other targets are `pkg[bin:name]`, `pkg[test:name]`, `pkg[example:name]`,
  `pkg[bench:name]` and `pkg[build:build-script]`. A `[test:...]` module has
  `test_of` set to the library. A `#[cfg(test)] mod` is a module of its own
  (a `tests` module inside its parent) with `test_of` set to the module around
  it, so its symbols count as test code for rules even though its file is not a
  test file; files under `tests/` and `tests.rs` are test files by convention.
  A function is a test when an attribute's whole path is one of `test`,
  `tokio::test`, `async_std::test`, `actix_rt::test`, `actix_web::test`,
  `sqlx::test`, `rstest`, `rstest::rstest`, `test_case`, `test_case::test_case`
  or the wasm-bindgen test attribute; `other::test` is not.
- Symbols: functions, `#[test]`/`#[tokio::test]`/`rstest`/`test_case` functions
  (`test`), structs, enums, unions and type aliases (`type`), traits
  (`interface`), named fields and enum variants (`field`), constants and
  `static`s (`const`, `var`). Everything declared in an `impl` or trait is a
  `method`, associated functions included, owned by the impl's type (or the
  trait). A trait impl method's id carries the trait with its arguments
  (`m::Meters::From<f64>::from`) so impls never collide; an impl for a type
  outside the project or a blanket impl has no `owner` and names the written
  type in the id. Functions declared inside a body are `function` symbols owned
  by the enclosing function. `macro_rules!` definitions are not symbols.
- Visibility: `pub` is `public` when every module up to the crate root is `pub`
  or the item is re-exported by a `pub use` of a reachable module, else
  `internal`; `pub(crate)`, `pub(super)`, `pub(in ..)` are `internal`; no
  keyword is `private`. Binary, test, example and bench crates are all `private`,
  like Go's `main`; the `pub` items of a library with `publish = false` are
  `internal` (like Go's `internal` packages) unless a sibling package depends on
  it, see `unpublished` above. Members are capped by their owner;
  enum variants and trait methods have their owner's visibility; a trait impl
  method has the visibility of the trait (and type).
- A trait impl method without docs shows the docs of the trait's method, as
  rustdoc does. Doc comments are `///` and `#[doc = ".."]` text.
- Edges: `calls` and `references` from function bodies to project symbols
  resolved through `use` trees (globs, renames, `self`, `super`, `crate`, local
  `use`), the `mod` tree and impl blocks: `foo()`, `module::f()`, `Type::assoc()`,
  `Self::f()`, `Trait::m(x)`, and `x.m()` when the receiver type is written in the
  code: `self`, typed parameters and `let`s (behind references, `Box`/`Arc`/`Rc`,
  `dyn Trait`, `impl Trait` and generic bounds), struct literals, calls of
  functions with a declared return type, and fields of known types. A method call
  on any other receiver emits no `calls` edge; it emits `heuristic` `references`
  edges to the non-`pub` inherent methods of that name in the same crate, unless
  more than three share the name, so a rule that counts callers fails safe. `implements` runs from a type to a project trait (not
  emitted for an empty impl in a file other than the type's). `imports` runs
  between modules, and to the crate name for dependencies outside the project.
  `accesses-private` is never emitted.
- Flow follows the model's mapping: `if`/`else if`/`else`, `if let`, `let ... else`
  (an `if`), `match` as one `switch` whose `arms` exclude a wildcard or binding
  arm and which is `returning` when every arm is a single value or `return` and
  the match is in return position (or all arms `return`), `loop`/`while`/`for`,
  labeled `break`/`continue` as `jump`, runs of `&&`/`||` as `logic`, a call of the
  enclosing function as `recursion`; `?` emits nothing; closures and `async`
  blocks raise nesting. Every `match` arm counts as a statement, like a Go case.
- Tests: a `table` test loops over a literal array or `vec!` of tuples or
  structs (directly or through a `let`), or carries several `rstest` `#[case]` or
  `test_case` attributes; others are `scenario`; `nesting` is always 0. `targets`
  are the calls and references of the body in order of first use.
- Macros are never expanded. The arguments of macro invocations are read as
  expressions when they parse as such (`assert_eq!`, `println!`, `vec!`,
  `matches!` and user macros taking expressions), so the calls inside are seen;
  anything else, `macro_rules!` bodies and code from `include!` are invisible.
  One notice per run counts the files that define `macro_rules!` and the
  invocations that were not read (item position, or arguments that are not
  expressions) with an example; `include!` has a notice of its own. Attribute macros and derives leave the item as
  written. A file whose header comment says `@generated` or `DO NOT EDIT` has
  `generated` set.
