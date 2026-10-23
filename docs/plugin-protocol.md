# Plugin protocol 0.3

Normative contract between Lighthouse (the host) and a language plugin: a
separate process that analyzes source in its own language with native tools.
Rules and analyzers stay in the host; a plugin only turns files into
the code model. The wire types are defined by
[`protocol/schema/lighthouse-protocol-0.3.json`](../protocol/schema/lighthouse-protocol-0.3.json),
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

`protocolVersion` is `"0.3"`. Host and plugin MUST agree exactly; a plugin that
receives another version answers `initialize` with an error, a host that
receives another version refuses the plugin. Receivers MUST ignore unknown
fields. Fields of protocol messages are camelCase (`protocolVersion`,
`clientInfo`, `workspaceFolders`); fields of the code model keep the model's snake_case names
(`test_of`, `max_nesting`).

`extent` on symbols, `site` on edges and the capabilities `extent` and
`reference-sites` were added to 0.1 before 1.0, as optional fields: a plugin that
sends none of them remains valid in 0.3, and hosts ignore what they do not
know. They make fixes possible (see "Fixing" in [architecture.md](architecture.md)).

`context.overlays` is real: a list of `{ path, text }` that stand in for requested
files. A provider MUST analyze the overlay text instead of the file on disk for those
paths and MUST NOT read the disk for them; the files on disk are never touched. A fix
run verifies candidate edits this way, and writes only after they pass. `lang-go`
passes them to `go/packages` as its `Overlay`, `lang-rust` reads them in place of the
files.

Reserved for later versions, not implemented in 0.3: further methods for rules
and analyzers.

## Methods

| Method | Kind | HTTP twin |
|---|---|---|
| `initialize` | request | none |
| `lighthouse/index` | request | none |
| `lighthouse/classify` | request | `POST /v1/systemone` |
| `lighthouse/train` | request | `POST /v1/fine_tuning/jobs` |
| `lighthouse/edit` | request | none |
| `shutdown` | request | none |
| `exit` | notification | none |

The table is `ROUTES` of `lighthouse-protocol`; a test fails when they differ. A method with an
HTTP twin takes and returns that API's body, so a server that speaks the API can stand in for the
plugin. A future method, added with its first consumer: `lighthouse/embed` (`POST
/v1/embeddings`).

### `initialize` (request)

The shape is MCP's, with LSP's names for the workspace. Params:

```json
{ "protocolVersion": "0.3",
  "clientInfo": { "name": "lighthouse", "version": "0.1.0" },
  "workspaceFolders": [{ "uri": "file:///abs/root", "name": "root" }] }
```

`workspaceFolders` holds exactly one folder for now: the project root, as a `file://` URI
percent-encoded as RFC 8089 and RFC 3986 say. A plugin turns the URI back into a path (`url`
in Rust, `net/url` in Go) and refuses anything else. Result:

```json
{ "protocolVersion": "0.3",
  "serverInfo": { "name": "lang-go", "version": "0.1.0" },
  "capabilities": {
    "languages": [{ "id": "go", "globs": ["**/*.go"], "priority": 0,
                    "conventions": { "test_globs": ["**/*_test.go"], "constructor_prefixes": ["New", "new"] },
                    "capabilities": ["semantic-edges"] }] } }
```

- `serverInfo.name` MUST equal the `id` of the plugin's manifest. `clientInfo` and `serverInfo`
  are one type, MCP's `Implementation`.
- `capabilities.models` (see "Model methods") is omitted when the plugin offers none.
- `globs` select project-relative files (`*` and `?` stop at `/`; a `**`
  component spans directories). Of several languages matching a file, the higher
  `priority` wins, then registration order; a `fallback: true` language only
  claims files no other language claims.
- `conventions.test_globs` mark test files for the whole host: rules skip or
  target them by this flag.
- `conventions.constructor_prefixes` are the name prefixes that make a function a
  constructor (a name is one when it is a prefix, or a prefix followed by a new
  word). Every rule that tells constructors apart shares them; a project replaces
  them once in `[languages.<id>] constructorPrefixes`.
- Each entry of `capabilities.languages` is the language's **provider manifest**
  (`ProviderManifest`): `id`, `globs`, `priority`, `fallback`, `conventions` and
  `capabilities`. It is what the host's `LanguageProvider::manifest()` returns, field
  for field; the plugin's own identity (`serverInfo`) is the plugin manifest.
- The `capabilities` of a language list what its provider guarantees. `semantic-edges`: `calls`,
  `references`, `implements` and `accesses-private` edges are resolved by the
  language's own semantic analysis (types, scopes), not by name matching. A rule
  that requires a capability is skipped, with a notice, for languages without it.
  `overlays`: the provider analyzes the `overlays` of an index request instead of the
  files on disk; a host declines fixes in files of a provider without it (bundled
  providers declare it).
  `complete-references`: every reference to a symbol, in signatures, fields and
  receivers too, is an edge with a `site`, so a rename cannot miss one; no bundled
  provider declares it yet.
  `extent`: every declaration symbol carries its `extent`. `reference-sites`: every
  edge that comes from a use in code, and every `imports` edge, carries its `site`. A fix that needs a capability
  the file's provider lacks is declined, never guessed.
  Hosts ignore capability names they do not know. A provider without
  `semantic-edges` is syntactic: its edges come from names (and, where marked,
  guesses), which makes its caller and reference sets lower bounds; no separate
  capability announces that.

### `lighthouse/index` (request)

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
`context.options` holds the `spec.languages.<id>` tables of `lighthouse.toml`
for every language, as JSON; a provider reads its own entry and MUST reject
unknown keys in it (as an `incomplete` entry, see below).

`context.cache` is `{ "dir": "/abs/root/.lighthouse/cache/<plugin id>/<binary digest>" }`, a directory
the host owns, named by the digest of the plugin binary, and the provider MAY keep derived
results in it between runs. It is absent when the host runs without a cache and when the
request carries `overlays`: their texts are not what is on disk. A provider reads and writes only inside it, writes
atomically (a temporary file and a rename), and answers exactly the same with the
directory empty, full or absent: the same fragments, the same notices (apart from one
saying the directory cannot be written), the same `incomplete` entries. An entry is
used only when its key matches, and a key covers everything the entry was computed
from: the provider build, the options, the toolchain, and the content of the files and
of what they depend on. Any doubt is a miss. A result with an `incomplete` entry is
never written. Conformance indexes every case cold, then warm, and compares the answer
with an uncached run byte for byte. The field is optional.

Result `{ fragments, notices, incomplete }`:

- `fragments`: one per analyzed file, sorted by path, each
  `{ file: { path, generated?, facts? }, modules, symbols, edges, functions, tests, comments }`;
  `facts` is what the provider says of the file by name, text to text, which a program names as
  `{file.<name>}` ([programs](programs.md#the-command-line)); the Rust provider states the `edition` of
  the crate that holds each file.
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

### Model methods

Lighthouse defines no model API of its own: each method takes the body of a published API,
verbatim, with that API's own field names.

A plugin may offer models instead of, or besides, languages. `initialize` lists them in
`capabilities.models`, a `ListModelResponse` with the OpenAI list shape `{object: "list", data: [Model]}`.
Each `Model` has OpenAI's `{id, object, created, owned_by}`, capabilities
`{classification | embedding | training: {supported}}`, and Lighthouse's `version` (changes
whenever answers for the same input may), `cost` (0 for a model that runs locally for free),
`deterministic` and `abstains`. The manifest lists the same ids in `provides.models`. The host
keys behaviour off these capabilities, never off names.

`lighthouse/classify` takes the body of TypeSafe's System One API (`POST /v1/systemone`),
https://docs.typesafe.ai/api, for one subject: `{state, model, questions, options?}`. `options`
holds provider configuration and is omitted when empty. HTTP clients remove it before sending the
System One body to the API.
- `state` is `{language, file, text, facts, neighbors}`: the subject's source by extent, the facts
  the check computed, and a few callers and callees.
- `questions` is keyed by decision id; each is `{type: "noul", instructions: {requirement, prompt},
  criteria: {true: {meaning, examples}, false: {meaning, examples}}}`. `true` means the subject
  fails the decision, and its examples are the decision's invalid examples; `false` means it
  passes, with the valid ones.
- `model` is the part of the reference after the provider (`openrouter/qwen/qwen3-coder:free`
  sends `qwen/qwen3-coder:free`). Beside System One's body (`SystemOneRequest`) Lighthouse sends
  `options`, the provider's `options`, for a plugin that needs them (`protocol`, `url`, `headers`, `body`);
  they are never part of the body a remote server gets.

It answers `{model, answers: {<decision id>: {type: "noul", noul}}, usage}`: `noul` is the
probability that the subject fails. There is no reason. The host maps it, the same for every
model: at or above the band's `upper` the subject fails (0.9 until `eval` calibrates one);
otherwise it is left to review. A model that has not been calibrated never clears a subject. The confidence is
max(p, 1 - p).

`lighthouse/train` takes OpenAI's fine-tuning job body (`POST /v1/fine_tuning/jobs`):
`{model, training_file, validation_file?, method: {type: "supervised", supervised:
{hyperparameters}}, seed, suffix?, metadata}`, and answers the final `FineTuningJob`. Here
`training_file` is the digest `sha256:<hex>` of a JSON Lines file in the host's dataset store,
`<datasets>/sha256/<hex>.jsonl`: one OpenAI chat fine-tuning line per subject (`messages`: the
system and user prompts and the assistant's `{type, noul}`), in canonical JSON, with `weight`,
`group`, `decision`, `key` and `attribution` beside them. `metadata` carries the `decision`, the
`datasets` directory and the `dir` the host owns for the artifact. A successful job
has `status: "succeeded"`, `fine_tuned_model: "learned@sha256:<hex>"` (or
`general@sha256:<hex>`, the artifact reference) and, as its first
`result_files` entry, a JSON file of the metrics. The same `seed` is the same request.

`lighthouse/edit` runs an agent: `{model, prompt, dir}` asks the model to change the files of the
directory `dir` as the prompt says, and answers `{said, events?}`. Only a model whose capabilities say
`agent: {supported: true}` serves it. `dir` is a scratch copy of the project that the host verifies
and deletes. The host does not send the request to the process it keeps for the other methods: for
each request it starts a process of the plugin of its own, sandboxed like a `command` agent (it writes
`dir` and its own `HOME`, reaches the hosts of the project's `[sandbox]`, and has no secret in its
environment), tells it `initialize`, `lighthouse/edit` and `shutdown` in one go and reads the answers
when it exits. `events` is what the agent did, when its harness tells: a list of `{tool, path?,
pattern?, command?}` with `tool` one of `read`, `search`, `list`, `edit`, `run`, and paths absolute or
relative to `dir`. The host reads them leniently: an event of another tool, or a malformed one, is
skipped and counted, never a failure of the plugin; at most 10 000 are kept; paths are made relative to
`dir`, and one outside it is dropped and marked; after the run the path of a `read` is kept only if it
names an existing file of `dir` (of a `search`, a file or directory), else the event stays without it; a
path through a symbolic link, and a `pattern` that is an absolute path outside `dir`, are marked like a
path outside. `said` is cut to its last 2 KB with secrets hidden. The host
reports them (see [models.md](models.md#what-an-agent-did)) and decides nothing by them.

Errors are JSON-RPC errors; `data` carries the error body of the API reused (OpenAI's
`{error: {message, type, param, code}}`, or TypeSafe's status code). Calls are retried with
backoff on 429, 503 and 529. A model that fails leaves its subjects to review.

## Errors and failure handling

A response MAY carry a JSON-RPC `error`, with the codes of JSON-RPC and LSP:

| Code | Name | Meaning |
|---|---|---|
| `-32700` | ParseError | malformed input (the plugin then stops) |
| `-32601` | MethodNotFound | unknown method |
| `-32602` | InvalidParams | the params do not fit the method |
| `-32603` | InternalError | a panic or a broken invariant of the plugin |
| `-32803` | RequestFailed | the request was valid and the plugin could not do the job; `data` is the error body of the API behind it, verbatim |
| `-32002` | ServerNotInitialized | any request but `initialize` before a successful `initialize` |

A plugin that receives another protocol version answers `initialize` with `RequestFailed`.
The host treats failures as follows, always turning them into
`incomplete` entries and never into a crash:

| Failure | Host behavior |
|---|---|
| `error` response to `lighthouse/index` | batch incomplete; plugin stays usable |
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
  kind = function | method | type | field | variant | const | var | interface | test
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
- `extent` (optional, capability `extent`): the whole declaration, from its first
  leading doc comment, attribute or annotation to its last token. Comments that lead it
  with no blank line between, and a comment that trails it on its last line, belong to
  it. A declaration that is one spec of a parenthesized group (a Go `const ( ... )`)
  has the spec's own lines, which a host never moves out of its group. A symbol whose
  declaration it shares with another (Go `var a, b = 1, 2`) has no extent. Moving or
  deleting a declaration takes exactly this range.

### Edges

`{ kind, from, to, resolution, site? }`. `from` is `{ "module": path }` or
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

`site` (optional, capability `reference-sites`) is the span of the identifier that names
the target at the place of use, so a host can rename it. The same relation used at
several places is one edge per place, and the host keeps one edge with every site. A
provider reports sites for the edges it emits from function bodies and for `imports`
edges, where the site is the import as written (a Go import spec, a Rust `use` item);
`contains` edges have none.

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

`params` counts the declared parameters, receiver excluded; `returns` counts the
results: every result in Go, and in Rust the arity of the returned tuple (1 for a
single value, 0 for `()`), looking inside a `Result<T, E>` at `T`.

`implementation` (optional) marks a function that implements a method a trait or
interface declares elsewhere: a method of a trait impl in Rust, a method of a type
that satisfies an interface its package names in Go. `constructs` (optional) marks an
associated function without a receiver that returns `Self` or its owner type, bare
or in a `Result` or `Option`.

`signature` (optional) is `{ params, results }`, the types of the parameters
(receiver excluded) and results, one `TypeRef` per occurrence in order (a Rust
tuple result is one entry). A
`TypeRef` is `{ text, symbol?, exported? }`:
- `text` is the type as written, normalized. Go prints it with `types.TypeString`
  and the full import path as qualifier (`context.Context`, `*example.com/m/x.T`,
  `[]string`); Rust prints the type's tokens without the spaces between them
  (`Option<Ctx>`, `&mut Builder`).
- `symbol` is the project type it names, as a kind-less symbol id (`module::Type`),
  behind pointers, references and aliases, so rules can ask
  whether a function takes a value of some type and find the struct that exists only
  for one function. A slice or map of the type names nothing.
- `exported` is whether that named type is exported, when `symbol` is set: Go
  `types.Object.Exported` of the name as written (an alias that exports an
  unexported type counts as exported); Rust: declared `pub`.

A field symbol carries the same `type_ref` for the type of the field, and may carry
`optional: true`: a caller may leave it out (Go pointer, slice, map, function,
channel or interface; Rust `Option`, `Vec`, a map, or a `#[serde(default)]` field).

`events` (optional) lists what a function body does that a rule may flag, in
source order, each `{ kind, span, detail? }`. The kinds are the same for every
language and each is reported only where the language has it; only a function with
a body has events, and a plugin that reports none leaves the field out. Closures
count for the function that contains them; functions declared inside a body have
summaries of their own.

| kind | maps |
|---|---|
| `panic` | Go: a call of the builtin `panic`. Rust: the `panic!`, `unreachable!`, `todo!` and `unimplemented!` macros; `detail` is the macro name |
| `unwrap` | Rust: a `.unwrap()`, `.expect(..)`, `.unwrap_err()` or `.expect_err(..)` call on any receiver, as clippy's `unwrap_used` and `expect_used` see it; `detail` is the method name |
| `error-compare` | Go: `==` or `!=` where one operand has type `error` and the other is not `nil` (or a `switch` on an error with a case other than `nil`), after go-errorlint's `comparison`; `detail` is the error operand. `io.EOF`, `sql.ErrNoRows` and the body of an `Is(error) bool` method are exempt |
| `error-assert` | Go: a type assertion to an error type, or a type switch, on an `error` value, after go-errorlint's `asserts`; `detail` is the asserted type, or the operand of a type switch |
| `errorf-unwrapped` | Go: `fmt.Errorf` with a literal format that has no `%w` and an argument that is an error, after go-errorlint's `errorf`; `detail` is the verb that formats the first such argument |

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

A plugin directory holds `lighthouse-plugin.toml` (or `.yaml` or `.json`: any
format reads the same document) and the executable. The manifest is a `Plugin`
resource, described by `schema/plugin.schema.json`:

```toml
apiVersion = "lighthouse/v1alpha1"
kind = "Plugin"

[metadata]
name = "lang-go"            # must equal the id the process reports

[spec]
version = "0.2.0"

[spec.runtime]
command = "./lang-go"       # relative path: next to the manifest; bare name: PATH
args = []

[spec.provides]             # optional
languages = ["go"]                 # checked against what the process reports
```

`provides` lists the `languages` and `models` the plugin offers, so tools can see them
without starting it; both are checked against what the process reports.

Only plugins listed in `lighthouse.toml` are started. Manifests that do not parse are ignored (with a notice) unless the directory
is named after a listed id. A bare id is looked up in
`./.lighthouse/plugins/*/`, `$XDG_DATA_HOME/lighthouse/plugins/*/` (default
`~/.local/share/lighthouse/plugins/*/`) and `<executable
dir>/plugins/*/`; an entry `{ id = "lang-go", path = "plugins/lang-go",
timeout = "120s" }` names the directory itself (relative to the config) and is how
a development checkout is used. A plugin the project names by a `path`, and one in
`./.lighthouse/plugins`, is code the repository brings and starts only in a trusted
project ([architecture](architecture.md#sandbox)); one in the user's plugin directory or
next to the executable needs no trust. The same id in more than one place, an in-process
plugin included, is an error: there is no shadowing.

## Trust model

Running `lighthouse check` executes the plugins that the project's
`lighthouse.toml` names, with the user's privileges, exactly as running a
project's build scripts does. There is no trust prompt or flag: check only
projects whose configuration you trust. Plugins found by search are started only
when listed; an unlisted manifest is never executed.

## Conformance

Every provider passes the golden suite in `crates/lighthouse-rpc/tests/conformance/<language>`: each
case is a project, option variants (`options[.variant].json`) and the exact
`lighthouse/index` result expected for them (`expected[.variant].json`), covering symbols,
edges, visibility, ownership, spans, flow, tests, capabilities and a
deterministic order. `cargo test -p lighthouse-rpc` runs the suite against the
built plugin and validates every result against the schema.

The exchanges of the protocol itself are recorded in `protocol/conformance/*.json`: each file
has `applies` (`language`, `model`) and an ordered list of `exchanges`, each a message to send
and the reply it must get (`expect`, `null` for a notification), where `"$any"` stands for any
value and `"$string"` for any string. The transcripts cover the lifecycle, a request before
`initialize` (`ServerNotInitialized`), an unknown method, another protocol version and invalid
params. Every plugin of the kind a transcript applies to replays it
(`crates/lighthouse-rpc/tests/it/transcript.rs`), and the Go sdk replays the same files
(`plugins/lang-go/sdk/conformance_test.go`), so the two servers cannot drift.

## `lang-go`

Reports `extent` (doc comments and the comment groups of the comment map that lead
the declaration without a blank line, and a comment that trails it) and `reference-sites`
(the identifier of every use `go/types` resolves, selector identifiers included, and
the import spec of every `imports` edge).

Loads packages with `golang.org/x/tools/go/packages` (tests included) and
type-checks with `go/types`.

- Cache: the unit is a package directory (all its variants, tests included). Its key
  is a digest of the provider build, the options, `go env` (version, GOOS/GOARCH, flags,
  workspace), go.mod, go.sum and go.work, the content of every file of the directory,
  the *API* of every project directory it imports, directly or not, and the interface
  declarations of the whole project. A file's API is its text without function and
  method bodies, so a body edit leaves every other unit's key unchanged (early cutoff,
  as in rustc's incremental compilation and Go's build action IDs, which hash the
  export data of dependencies rather than their sources). Only the changed
  directories are loaded; the rest of the packages come from export data, as with any
  `go/packages` load without `NeedDeps`. Where the unit's own API and its imports'
  are unchanged, its old `implements` edges are reused; otherwise the directories that
  declare interfaces are loaded too, so types are matched against all of them. A
  `vendor` directory and the targets of local `replace` directives are fingerprinted
  by file names, sizes and modification times. A unit with an `incomplete` entry is
  analyzed again on every run.
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

Reports `extent` (from the first outer attribute or doc comment, or the plain
comment lines above them, to the last token) and `reference-sites` for the edges it
resolves, and the `use` item of every `imports` edge; they are syntactic or heuristic,
so a host does not rename through them.

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
- Cache: the unit is a file. Parsing, the module tree and name resolution always run
  (a parse is about half of a run, name resolution about 2%); the symbols, edges and
  summaries of a file come from the cache when its text and its context are unchanged.
  The context is a digest of the options, of every `Cargo.toml` above the requested files
  and of every source file the module tree reached, without the bodies of its functions.
  A body edit therefore analyzes one file; any other edit analyzes all of them again. A
  tree with a parse error or a missing file is not cached.
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

## `model-http`

Models behind the standard HTTP protocols, as a provider in `lighthouse.toml`
(`protocol = "openai"` or a list such as `["openai", "typesafe"]`, `url`, `headers`, `body`; see
[rule-pipeline.md](rule-pipeline.md#models)). Each operation uses the protocol that covers it:
classification uses `typesafe` when listed, else `openai`. There is no chat method: Chat Completions
are only the logprobs adapter behind classification.

For `typesafe`, the System One body is forwarded as it is to `POST {url}/v1/systemone` with the provider's `headers` (`Authorization: Bearer <key>` at the service of the protocol) (https://docs.typesafe.ai/api), and its answer is read as it comes.

For `openai`, each question is rendered as a prompt that asks for one word, `true` or `false`, and
sent to `{url}/chat/completions` with the provider's `headers`, `temperature` 0,
`logprobs`, `top_logprobs` 20 and `max_tokens` 1. P(fail) is read from the probabilities of the
two words at the answer token. A response without logprobs is an error ("the model returned no
logprobs; it cannot serve classification"), never read as a verbalized number. The API shapes are
those of OpenAI's Chat Completions as documented by OpenRouter
(https://openrouter.ai/docs/quickstart).
