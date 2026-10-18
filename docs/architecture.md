# Architecture

```
 CLI ─ engine ─ config ─ reporters ─ store (.lighthouse/lighthouse.db)
          │      └─ fix orchestrator (the only writer)
   plugin registry ── in-process plugins: core, metrics, design, testing,
          │            and `local` (declarative rules of the project)
          │
   RPC host (lighthouse-rpc) ── language plugins as processes
          │                      plugins/lang-go (go/packages + go/types)
          │                      plugins/lang-rust (syn + own module resolution)
   code model (UCM, lighthouse_model::ucm)
```

The core knows no language and no rule. Languages arrive as providers that turn
files into the code model (UCM: symbols, edges, function summaries, tests);
analyzers and rules read only the UCM. In-process providers and out-of-process
plugins (see [plugin-protocol.md](plugin-protocol.md)) implement the same
`LanguageProvider` contract, which is project-level: the engine calls each
provider once with all of its files.

## Plugin kinds and manifests

A plugin is a contract boundary, not a process boundary: bundled Rust code and
out-of-process plugins satisfy the same traits. Every kind describes itself through
`manifest()`, a static data struct, and keeps behavior in separate methods, the way
`lighthouse-plugin.toml` describes a plugin process:

| Kind | Manifest | Behavior |
| --- | --- | --- |
| `Plugin` | `PluginManifest { id, version }` | `languages`, `analyzers`, `rules`, `fixers`, `order_keys` |
| `LanguageProvider` | `ProviderManifest { id, globs, conventions, capabilities, fallback, priority }` | `index` |
| `Analyzer` | `AnalyzerManifest { id, requires, scope }` | `run` |
| `Rule` | `RuleManifest { id, severity, scope, description, docs, analyzers, capabilities, applicability }` | `validate`, `check` |
| `Fixer` | `FixerManifest { id, requires }` | `fix` |
| `OrderKey` | `OrderKeyManifest { id, description }` | `rank` (runtime only: no resource) |

The `initialize` result of a plugin process is its `PluginManifest` plus one
`ProviderManifest` per language, field for field (see
[plugin-protocol.md](plugin-protocol.md)).

Dependencies point down only (see [Crates](#crates)): `spec` and `plugin` never depend on each other, `plugin` knows no configuration, and protocol stays dependency-free.
The wire types are separate from the UCM: renaming a UCM field is not a
protocol change. The bridge from a decision to its `Rule` (`rule_manifest`, option
resolution) lives in `checks`; `spec` only describes. `engine` takes the catalog it runs
over, for the projects `extends` names and for which decisions are in force.

## Analysis scope and report scope

They differ on purpose. `check [paths]`, `--changed` (working tree against HEAD,
untracked files included) and `--diff <base>` (since the merge base with `<base>`),
and later hook file scopes, only filter which diagnostics are reported. Analysis always
covers the whole project, because a rule about a function needs its callers, a
provider needs the whole package and build context, and a finding in one file may
depend on another. A gap anywhere in the analysis is reported whatever paths were
requested.

A file-level filter is all there is for now: a finding in a changed file is
reported even on a line the change did not touch. Files the project never wants
analyzed (fixtures that are broken on purpose) are listed in `.lighthouseignore`,
in `.gitignore` syntax; unlike a report filter, that removes them from analysis.

## Terms

| Term | Means |
| --- | --- |
| **decision** | What was decided and why: the catalog entry (`Decision`), with its severity, check, options, examples and fix. The one authored concept. |
| **rule** | The executable form the engine compiles from a decision's `check`: the `Rule` trait, the rule ids that findings carry (a rule id is the id of its decision). |
| **judgment** | A recorded label on one subject, as SARIF's `result.kind` says it: `pass`, `fail` or `notApplicable`. Not ground truth. It belongs to a decision's meaning version and is attributed (PROV `wasAttributedTo`, `generatedAtTime`). |
| **suppression** | A finding that is right and is left in place on purpose, as SARIF says it: `inSource` (a directive in the code: `lighthouse-disable-next-line <rule> -- <reason>` or another form) or `external` (recorded with a `fail`), with a `justification`; `status` is `accepted` unless said otherwise. |

## Resource model

Every spec document has one envelope, in the style of the Kubernetes resource
model, and is read from YAML, TOML or JSON alike (a YAML file may hold several
documents):

```yaml
apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: core/file-size        # the id, `<namespace>/<kebab-name>`
  uid: 8a4f0a2c-6b57-4a8e-9a42-0d2f3b9f5c11   # identity, a UUID v4 assigned once
  annotations: {}
spec: { ... }
```

| Kind | Holds | Lives in |
| --- | --- | --- |
| `Decision` | a decision: `title context scope requirement status supersedes consequences severity options languages check fix provenance examples` | `decisions/<pack>/<section>/<name>.yaml`, `.lighthouse/decisions/` |
| `Pack` | title, intro and the ordered sections, each listing its decisions | `decisions/<pack>/pack.yaml` |
| `SourceMap` | which decision covers each normative line of the documents a catalog came from | `decisions/sources.yaml` |
| `Project` | `lighthouse.toml`: `plugins extends languages rules overrides generated`; also a shareable configuration, which `extends` names like an ESLint shareable config. `<pack>/recommended` and `<pack>/strict` are Projects derived from the decisions (`strict` also switches on every kind of a decision whose `kinds` option enables only some by default); a catalog layer may hold more | project root, `.lighthouse/decisions/` |
| `Plugin` | `lighthouse-plugin.toml`: `runtime {command, args}` and `provides` | plugin directory |
| `Judgment`, `Suppression` | records of the decision log | `.lighthouse/decisions.jsonl` |

A decision's `metadata.uid` (Kubernetes `metadata.uid`) is its identity; the name is
only what people call it. It is a random UUID v4, assigned once and never derived from
the name, so renaming a decision touches no record: fingerprints and judgments are
seeded by the uid. `decision_create` assigns one, and `spec validate` asks for one and
refuses two decisions with the same.

Conventions: keys are lowerCamelCase, option names included (`maxWords`), durations are strings (`30s`), enums are lowercase kebab,
paths are relative with `/`. A decision belongs to the pack its id names and to
the section of that pack's file that lists it, so where the file lies carries no
meaning. `scope` is `{subject, generated, tests}` (`generated: true` makes generated code a subject, default false; `tests: exclude|include|only`, default `exclude`, set explicitly where a decision is about tests or covers them). The host tells generated code apart once (the provider's marker such as Go's `// Code generated ... DO NOT EDIT.`, `.gitattributes` `linguist-generated`, the project's `[generated] files`) and the engine leaves out the subjects the scope excludes before any check runs, so no expression guards against `generated` or `test`. A project overrides it with `[generated] check = "skip"|"include"` for all decisions, or `rules."<id>".generated = true|false` for one. A `cel` check's `select` defaults to the scope's subject. `options` is a JSON
Schema object (`type`, `default`, `description` per property, closed with
`additionalProperties: false`); `languages.<id>` holds the option values and the
wording of one language. `check` and `fix` choose a provider by `type`: `check` is
`builtin`, `cel`, `command` or `model`; `fix` is `ops`, `command` or `rpc`, with the fields of its type
next to `safety` and `requires`, and no others.

The JSON Schema (2020-12) of each kind is generated from the Rust types into
`schema/` and checked in (`lighthouse schema [kind]` prints one, `--write DIR`
writes all; a test fails when the files are stale). Documents carry a
`# yaml-language-server: $schema=...` comment so editors validate them.
`lighthouse spec validate [paths]` checks each document against its schema and
against the others (extended projects and rules name things that exist, fix operations name
registered order keys, CEL compiles, examples are well formed; `--examples` also
runs them). Readers accept the current format only.

### Configuration layers

Meaning is shared; the environment is personal. The keys that decide findings, or what may change
the shared repository, are the project's alone (`lighthouse.toml`, committed): `plugins`,
`extends`, `languages`, `overrides`, `generated`, `[fix]`, and per decision `level`, `options`,
`generated` and `evaluation`. The keys that say how this machine computes and applies are the
environment: `[models]` (bindings, `providers`, `routing`, `budget`, `explore`),
`rules."<id>".classification` and `rules."<id>".edit`, and `[approval]`. They are read from four
layers, the later winning key by key:

| Layer | File |
| --- | --- |
| bundled | the defaults |
| project | `lighthouse.toml` |
| user | `$XDG_CONFIG_HOME/lighthouse/config.toml` (default `~/.config/lighthouse/config.toml`), every repository |
| local | `.lighthouse/local.toml`, this repository; `lighthouse init` adds it to `.gitignore` |

With `--config FILE` the project root is the current directory, so the local file is
`.lighthouse/local.toml` under it, not beside `FILE`. Git must not track the local file: when it does,
loading fails until it is untracked (`git rm --cached`). `config show` hides what may carry a secret
(the values of a provider's `env`, `options` and `extraBody`, and the userinfo and query of its
`baseUrl`; the `command` is shown as written), and errors in a personal file give its line and key but
never the text of the line. Trust covers the part of the personal files that can run a program or
write without asking (a provider with a `command` and its `env`, and `[approval]`), not their budgets
or bindings.

The user and the local file are bare TOML with the environment keys only
([environment.schema.json](../schema/environment.schema.json)); a meaning key in either is a load
error that names the key and the file. A binding (a reference or a route list) is replaced, never
joined; a provider is replaced whole by a later layer that defines the same name. A decision a local
file binds must exist. Trust stays the user's (`trust.toml`) and covers the personal files too: a
change to one withdraws it, and a command provider they declare runs only in a trusted project.
Findings of deterministic checks are the same for everyone; only a model's answers can differ, and
they are cached by model. `lighthouse config show [--origin]` (MCP `config_show`) prints each
environment key with its effective value and, with `--origin`, the layer it came from, the way
`git config --show-origin` does; CI sees the project layer alone unless it writes the other files.

## Severity

A finding is `error`, `warn` or `info` (and a rule may be `off` in
configuration). A decision authors its severity (`severity: error|warn|info`); a decision without a `check` is documentation and yields no finding. A project's `level` changes what is reported and the exit code, never the authored severity. The same
three levels map to SARIF (`error`, `warning`, `note`). Exit code: `1` if any error;
a warning fails only with `--strict` or past `--max-warnings N`; `info` never
fails; an incomplete analysis is `3`.

Needing review follows the *authored* severity: `error` is definitive (no review, only a directive in the code suppresses it, its fix may be `safe`); `warn` and `info` are review tasks whatever level the configuration reports them at, until a judgment stands for them (`needs_review`: a non-error authored severity and no judgment).
`review list`, the MCP `review_list` tool, the JSON output and the hooks select by
that property. A rule that has no decision is treated by its severity.

Hiding follows the same property, not the severity: a judgment can hide a finding of a
`warn` or `info` decision even when the project configured it as `error`, and never one
of an `error` decision even when configured as `warn`. So an agent is only asked for a
judgment that can take effect.

## Rules

Every implemented rule belongs to a catalog decision whose `check` is
one provider of the union `builtin | cel | command | model`, with the common fields `requires` and `timeout`:

- `builtin` is a standard, decision-agnostic operation (`order`, `cycle`) or, for the two rules the engine reports itself (`core/allow`, `core/no-unused-allow`), a rule named by `id`.
- `cel` is an expression over the code model (UCM), written in the decision, with the standard function library `metrics callers owner tests rank exposed limit counted`, the module path helpers `globMatch layerOf` and the text helpers `lines trim join trimPrefixes trimSuffixes trimLeft trimRight leadingRun drop`.
  - A function node has a `role`, computed once for every rule, the first that fits: `test`, `implementation` (it satisfies an interface or trait), `constructor` (named by the language's `constructorPrefixes`, or a Rust associated function returning its owner), `entrypoint` (Go `main`/`init`, Rust `main` of a binary), then `method` or `function`. The test sub-role of a test-file declaration is `test_role`.
  - A limit option is declared `$ref: '#/$defs/limit'`: an integer for every role, or an object with `default` and one entry per role (`function method constructor implementation entrypoint test`), each an integer or null for no limit. `limit(node, max)` gives the value for the node's role, and `counted(node, n, what)` says it (`NewServer needs 9 parameters`, `parse has 9 parameters`).
- `command` runs a program under the process contract: argv without a shell, `{file}`/`{files}`/`{rule}` filling whole arguments, `batch: file|all`, exit `0` clean, `1` findings (stdout lines, an optional `path:line[:col]: ` prefix), anything else an execution error that leaves the analysis incomplete (exit 3). It runs only in a project the user trusts (`lighthouse trust`), and the trust covers the program and every argument that names a file inside the project, by content, so `sh script.sh` is bound to the script. A command is not sandboxed: it runs with the user's privileges, in the project root, with an environment cleared to `PATH`, `HOME`, `LANG`, `TMPDIR` and the declared `env` plus `LIGHTHOUSE_*`. Trust is the only protection; a command that exits with an execution error, times out or prints more than the output cap leaves the analysis incomplete, never clean. `output: sarif` reads stdout as a SARIF log instead, which is how common linters are wrapped: see [wrapping linters](wrapping-linters.md).
- `model` asks the `classification` task of the model the project binds to the decision (`select` and `prompt` optional); a subject nothing decides stays a review task. It is not deterministic and caps the severity at `warn`.

A decision with no `check` is documentation. How a decision is checked is not part of what it means: the *meaning version* hashes requirement, severity, scope (with its applicability) and options, and the `check_revision` records the check on findings and judgments without ever expiring one.

```yaml
check:
  type: cel
  select: symbol            # symbol | function | edge | module | file | test
  where: 'symbol.kind == "var" && symbol.visibility != "private"'
  message: 'exported variable {{ symbol.name }} is mutable package state'
  evidence: { symbol: symbol.id }
```

`select` binds one variable of the same name (`func` for `function`, which CEL
reserves) with the fields of that kind: symbols have `id name kind visibility
owner owner_kind file line module lang documented test generated callers callees
references members`; functions add `statements top_level params returns
max_nesting tokens branches`; edges have `kind resolution from to` (each end with
`kind id module name`); modules `path name test_of files symbols lines dependents declares`; files `path
lang test generated lines symbols functions`; tests add `nesting style targets
target_count`. Absent values are empty strings, never null. `edge` and `module`
rules run once over the project, the others once per file. Projects add their
own decisions as `.lighthouse/decisions/*.yaml` (a `Decision` with id `local/<name>`
and a `cel` check), enabled through the `local` plugin of
`lighthouse.toml`. `lighthouse decision test [ids]` runs every example of the
bundled and local decisions through the whole engine, in each language whose
plugin the configuration lists.

Language specifics stay out of the rules: provider facts (symbols, owners,
visibility, spans, comments) are the same everywhere, and per-language option
defaults in the decision realize a rule per language; the context says what each language makes of it.

## Incomplete analysis

`Run.incomplete` is first-class: "not checked" never equals "passed".

- **Incomplete**: something inside the analysis scope or the requested scope was
  not analyzed. Unreadable files; files a language provider could not parse, load
  or type-check; a provider that crashed, timed out or sent malformed data; a
  walk error; a requested path outside the project root; a non-UTF-8 file claimed
  by a language provider (a Go source that is not UTF-8 does not compile).
- **Notice**: the analysis is complete but the user should know. A rule skipped
  because the language lacks a required capability; build-variant duplicates and
  ambiguous edge targets found while merging; files excluded by the configured
  build context; messages a plugin wrote to stderr; non-UTF-8 files claimed only by
  a fallback provider (binary data).

`lighthouse check` exits 3 when the analysis is incomplete, before findings are
evaluated; `--allow-incomplete` exits by findings only (for hooks that must not
block) and still prints the gaps. Text output prints `incomplete:` lines, JSON
output `{"incomplete": …}` lines, and SARIF sets the run's invocation to
`executionSuccessful: false` with one `toolExecutionNotification` per gap.
Text output ends with a `summary:` line of counts (errors, warnings, reviews,
incomplete) whenever there is anything to report. A configured plugin that starts
but crashes, times out or answers badly during `initialize` makes the run
incomplete; one that cannot start or speaks another protocol version or identity
is a configuration error. Running `check` executes the plugins the project's
config names, like build scripts (see the trust model in
[plugin-protocol.md](plugin-protocol.md)).

Exit codes: 0 clean, 1 findings, 2 usage or runtime error (a configured plugin
that cannot start is one), 3 incomplete.

## Speed and determinism

A run does the work that is independent in parallel and the rest once. Language
providers index side by side (`lang-go` also loads its nested modules
concurrently); files are checked in parallel, each by every rule that applies to
it, and project-scope rules in parallel with them. What each step returns is
gathered in a fixed order (files in path order, rules in id order), so findings,
fingerprints and exit codes are the same on every run whatever the schedule;
fingerprint assignment runs after the findings are collected and
ordered. What several rules would compute alike is computed once per run and
shared through `Context::memo`: the base facts of a symbol that CEL checks read, and
the versions of a decision that the store records with each finding. Set
`RAYON_NUM_THREADS=1` to run on one worker.

Language providers keep what they derive from a file or package in
`.lighthouse/cache/<plugin id>/`, a directory the host passes in `context.cache`
(see the protocol). The cache is local and derived: never committed, always safe to delete, and
bounded in size (oldest first). Every key covers the provider build, the options and
the toolchain, and any doubt is a miss. A warm run must answer exactly like a cold one,
which conformance checks over every fixture. The model is the build systems that proved
it: Go's build action IDs (a content hash of the inputs plus the export data of the
dependencies, so a dependent is rebuilt only when what it sees changed), rustc's
incremental compilation with the red-green "early cutoff" of Salsa (a dependent is not
recomputed when an input's output did not change), ESLint's `--cache` (file content plus
configuration) and Bazel's action cache. For the dependents the "output" is the API, the text of
a package without function bodies, so a body edit re-indexes one unit.

`lighthouse check --timings` prints where a run's time went to stderr, one
`timings:` line per phase: setup, read, each provider's index, merge, each
analyzer, the rules (wall clock, then the five slowest rules summed over the
files they ran on), fingerprints, store, report and the total.

### Result cache

The model is that of Go's build cache, rustc's incremental compilation (Salsa's
early cutoff), ESLint `--cache` and Bazel's action cache: a result is kept under
a key of everything it was computed from, and any doubt is a miss. The cache is
`.lighthouse/cache/` (derived, never committed); `lighthouse-engine` (its `cache`
module, over rusqlite) owns `results.db`, a WAL SQLite table of findings and notices by key,
capped at 256 MB (`LIGHTHOUSE_CACHE_LIMIT_MB`), least recently used pruned at the
end of a run. `check --no-cache` (MCP `noCache`) bypasses it; `lighthouse cache
clean` deletes the whole directory, providers' subdirectories included.

A rule declares its **reach** (`checks::reach`, next to the facts and functions it
describes; a test fails for a fact or function without one): `local` (the
subject's own symbol, file and summary), `neighbors` (one edge, owner or member
away) or `global` (indexes and walks over the project). A rule's reach is the
greatest of what its expressions mention; project-scope rules, `cycle` and rules
that report at another symbol are global; command, annotation and plugin rules
are not cached. A key holds the build, plugins and language conventions, the rule
id, its check revision (a hash of the decision), the resolved options and
applicability, then by reach the file path and its slice digest (local), plus its
neighbor digest (neighbors), or the digest of the whole merged project (global).
The digests are computed once per run, in parallel per file, after the merge. A
hit returns the stored findings and notices; failed or incomplete rule runs are
never stored. Fingerprints, suppression and reporting run afterwards on the full set,
so a warm run prints what a cold one does, byte for byte.

## Memory: findings, judgments and the feedback loop

Memory has two parts with different owners. **Sightings** (what `check` saw, when, with
which facts) stay on one machine, in `.lighthouse/lighthouse.db` (SQLite, the
`lighthouse-store` crate). **Decisions** (judgments and suppressions on findings) are shared
through a committed file, `.lighthouse/decisions.jsonl`, and the database holds a copy of
them that it rebuilds from the file. See [Shared decisions](#shared-decisions).

`lighthouse init` ignores `.lighthouse/*.db*` and marks the decision log `merge=union`
in `.gitattributes`. A run is recorded by default whenever the project has a
`lighthouse.toml`; `--no-store` runs without reading or writing the store and applies no
judgments. A store that cannot be used never fails a check: the run goes on, says so on
stderr, and still applies the judgments it can read. The database uses WAL with
`synchronous=NORMAL`, writers take the write lock up front (`BEGIN IMMEDIATE`) and wait
for each other, and opening is serialized the same way, so a hook, a stop script and a
human can run at once. The database is a cache with one schema and no migrations: its
version is `PRAGMA user_version`, and a database of another version, or one that has tables
and no version, is emptied and filled again from the log (only the history of sightings is
lost). A file that is not a
database is named in the error and left where it is (move it aside; it is rebuilt from
the log).

The store is artifact-neutral: a finding's artifact is a path string and its locator is
JSON (`{"span": {...}}` for text, a JSON pointer or region id for other artifacts), and
evidence, facts and options are JSON, so nothing in it is specific to code.

### Findings

`findings` has one row per fingerprint: rule, last severity and authored severity, path, locator,
owner symbol, `first_seen`, `last_seen`, `resolved_at`, `inactive_at`, how often it came
back (`reopened`), and the last sighting: message, evidence, facts, the options the rule
ran with, the commit and whether the tracked files were dirty, the Lighthouse and
catalog versions, the meaning version of the decision and a digest of the evidence. The
facts are what the analysis knew about the subject: language; for a symbol its kind,
visibility, owner, callers (split into same-module and other-module), callees, function
summary and the measures analyzers took (cognitive, cyclomatic, fan-in and fan-out,
size); for any other finding the file's size and the measures taken of the file. They are
kept per finding so a judgment can snapshot them without re-analyzing.

A run updates `findings` like this:

- every reported finding is inserted or refreshed, a resolved one is reopened;
- a finding that was open inside the run's *report scope* and rules and is absent now is
  marked resolved. `--changed` and `--diff` include deleted files in the scope, so what a
  deleted file had is resolved;
- nothing is resolved outside the report scope (`check src/a.rs`, `--changed`, `--diff`
  narrow it), for rules that did not run (`--decision`), or under a directory that holds a
  file the run could not analyze. A gap that cannot be placed (a language provider that
  failed) stops all resolving. "Not checked" never means "fixed";
- an open finding of a rule the configuration no longer enables becomes `inactive`: it
  is not open, not resolved, and listed with `review list --status inactive`. It is
  reactivated if the rule is enabled again.

`review prune [--older-than DAYS]` deletes resolved and inactive findings that nobody
judged; findings with judgments stay, because the judgments are labels.

### Fingerprints

A fingerprint is the decision's uid (not its name), the owner symbol's path (`module::owner::name#kind`) or the
file for file-level rules, and a whitespace-normalized snippet where the finding has
one. Line numbers are not part of it, so edits above a finding do not change its
identity. When findings of one rule in one file still share a fingerprint, the engine
tells them apart by the finding's symbol, or the symbol that encloses or precedes it,
and only the ones that still collide get an ordinal. Identities that rest on an ordinal
are marked in the facts (`ordinal`), and `review resolve` warns about them: the ordinal
moves when an identical finding appears before it. A finding that stops colliding goes
back to its undistinguished fingerprint.

**Seeding by uid.** The decision's uid seeds the hash, so a rename moves no fingerprint
and no judgment. A rule that has no decision (and so no uid) is seeded by its id.

### Meaning version

`meaningVersion` is the hash of a decision's normalized semantic content (requirement with
whitespace squashed, severity, scope, and the option types and defaults including each
language's values; never the `check`): the envelope, the labels, the file format and the
prose do not change it. A judgment belongs to the meaning version it was given under and
stops standing when the decision's moves.

### Judgments

A judgment is a recorded label on one finding (a subject): `pass` (the code conforms: a
false positive), `fail` (the finding is right) or `notApplicable` (the decision does not
apply here, a hint to narrow it). The first two label the check's conformance part, the
last its applicability part, and they are never mixed in metrics. A record holds the
finding's fingerprint, the decision's name then (kept to be read) and `decisionUid` (what
identifies it), `meaningVersion`, `checkRevision` (a hash of the `check`, recorded for
evaluation and never compared to expire a judgment), `decisionHash` (the whole decision
definition), `catalogVersion`, the Lighthouse version, the judgment, a free-text `reason`,
`wasAttributedTo` (`{type: Person | SoftwareAgent, id}`), `generatedAtTime`, language,
scope, a digest of the evidence, a **snapshot** frozen at judgment time, and the commit.
The snapshot is one JSON object, versioned (`"v": 2`): message, path, locator, symbol,
evidence, facts, options, severity, `authored` (the severity the decision authored), when
the finding was seen (`seenAt`), the commit and dirtiness at that sighting and the
Lighthouse version. The snapshot is the single owner of the frozen evidence.

A `fail` may go with a **suppression**: the finding is right and is left in place on
purpose. `review resolve --judgment fail --suppress <justification>` records an `external`
suppression, with status `accepted`, in the same transaction and the same breath as the
judgment; a suppression goes with a `fail` only. A directive in the code is the
`inSource` kind of the same type.

| Judgment | Suppression | Effect |
| --- | --- | --- |
| `fail` | none | the finding is right; stays visible, no longer asks for review |
| `fail` | `external`, accepted | the finding is right and left in place; hidden while valid |
| `pass` | | a false positive; hidden while valid |
| `notApplicable` | | the decision does not apply; hidden while valid, flagged to narrow the decision |

The reviewer is `--attributed-to person|agent[:<id>]`, else `LIGHTHOUSE_ATTRIBUTED_TO`,
else a person and the name configured for the repository or `$USER`. `person` and `agent`
are the PROV classes `Person` and `SoftwareAgent`; another word is refused. Tools that
run reviews for an agent (hooks, an MCP server) set the variable, so `SoftwareAgent` is
recorded without the agent having to remember it. `review resolve --seen <last seen>` refuses the judgment when
the finding has been seen again since the reviewer read it, and the finding is read in the
same transaction that records the judgment. `review resolve` works without a readable
catalog: it warns, and the versions are left out.

**What a judgment does is derived**, never stored, from the judgment that stands for the
finding (the stronger attribution first, a person over an agent; then the later
`generatedAtTime`, then the record id; see [rule-pipeline](rule-pipeline.md#judgment-and-suppression)).
It is matched to the findings of the run by fingerprint, meaning version and evidence
digest, so it holds on a clone with an empty findings table. A hiding judgment (`pass`, `notApplicable`, or `fail`
with an accepted suppression) hides the finding only while it is valid:

- the finding is not an error: findings of `error` decisions are decided by the rule,
  whatever severity they are configured at, so a judgment on one is recorded but never
  hides it, and `check` says so (`judged <judgment> - errors are definitive ...`);
- the meaning version of the decision is the one the judgment was given under (otherwise
  `judgment expired: decision changed`);
- the digest of the finding's normalized evidence is the one the judgment was given under
  (otherwise `judgment expired: evidence changed`).

An expired judgment leaves the finding in the report with a note, and asks for review
again. A `notApplicable` judgment also hides and is flagged as a hint to narrow the
decision: `review list --status narrowing` lists those. `check` prints how many findings
judgments hid.

**Labels** are what a judgment teaches later models about its decision: `fail` is
positive; `pass` and `notApplicable` are negative; a `fail` left in place by a suppression
is a separate target that says nothing about precision. A finding nobody judged has no
label and is never a negative.

## Shared decisions

Three things decide that a finding is not reported, with different lifetimes:

| | Lives in | Shared by | Holds while |
| --- | --- | --- | --- |
| Judgment, external suppression | `.lighthouse/decisions.jsonl` (committed) | git | the decision's meaning version and the finding's evidence are unchanged; never for errors |
| Source directive (`inSource` suppression) | a comment in the code | git, in the diff | the comment is there; reported when it suppresses nothing |
| Sighting history | `.lighthouse/lighthouse.db` (ignored) | nobody | it is a local cache |

**The decision log** is the source of truth for judgments. `review resolve` appends one
line per record, a `Judgment` record of the resource model (`apiVersion`, `kind`,
`metadata.name` = the record's id, `spec` with camelCase keys; canonical, compact, keys
sorted, so diffs are one line) and, with `--suppress`, a `Suppression` record that names
the judgment's id, each with a single write and `fsync`, then updates the cache. The `id`
is a hash of the spec. No other kind is read: a line of another kind, an unknown
`apiVersion`, a line whose id does not match its content or one that is not valid stops
the store with the file and the line number, so a decision is never skipped silently.
Opening the store imports the records the cache does not have and ignores the ones it has,
so the cache can always be deleted and rebuilt, a teammate's judgments apply as soon as
the log is pulled, and CI hides what developers hid. Two branches that both appended merge
by keeping both lines (`.gitattributes`: `merge=union`); the same record twice is one
record. The log is the only source of truth and the cache is derived from it: opening
the store (under the write lock, after reading the log) makes the cache hold exactly the
log's records and never writes to the log, so a branch switch or a removed line shows only
what the log there says. `review resolve` appends the judgment and its suppression in one
write, then updates the cache. A last line an interrupted write cut short is skipped with a
notice, and the next append cuts it off; a suppression that goes with no judgment of its
finding is ignored with a notice. Records are never edited by Lighthouse; a later or
stronger judgment replaces the standing of the finding.

**Source directives** put the decision next to the code it is about, where a reviewer
sees it change. They are directives in ESLint's forms, in a comment line that starts with
the marker (the description follows ESLint's ` -- `):

| Directive | Reaches |
| --- | --- |
| `lighthouse-disable-next-line <id>[, <id>] -- <reason>` | the symbol it documents, the line after it and its own line |
| `lighthouse-disable-line <id>[, <id>] -- <reason>` | its own line |
| `lighthouse-disable <id>[, <id>] -- <reason>` | from the comment to a matching `lighthouse-enable`, or to the end of the file; above the first symbol of a file, the whole file |
| `lighthouse-enable <id>[, <id>]` | ends the ranges of the named ids |

Ids are separated by commas and the reason follows ` -- `, as in ESLint
(`lighthouse-disable-next-line design/a, design/b -- why`). "The top of a file"
is above the first line that is neither blank nor a comment, so a header
comment may precede the directive and an import may not. A documentation
comment (`///`, `//!`, `/** */`) never holds a directive. `-next-line` counts
from the directive's own line; stacked directives each look at the line below
themselves, and only one on the last line of its comment reaches the symbol
declared there.

There is no form without ids. A directive applies at every severity, errors included,
because it is reviewed in the diff, and the report counts what it suppressed (`N allowed`);
it is a suppression of kind `inSource`, the same type a reviewer's `external` one has, and
SARIF lists the suppressed finding with it. The reason is required on every form that disables: a
directive without one is ignored and reported by the error rule
`core/allow`. A directive or range whose rule no longer fires there, or is not
enabled, an `lighthouse-enable` that closes nothing and a directive that names no id are
reported by `core/no-unused-allow` so annotations do not rot (when `--decision` leaves the
annotated rule out of the run, nothing is said). A second `lighthouse-disable` of an id
inside its open range is redundant and reported the same way. Both rules belong to
the `core` pack, so `core/recommended` enables them; the engine evaluates them because
whether a directive is used depends on every other rule's findings. Prose that merely
mentions a marker mid-line is not a directive. The comments come from the language
providers, which report them with their text and the symbol they document; a comment
of several adjacent line comments may hold several directives, one per line.

### Agent output

`check --format json` groups findings by decision, then by file: a decision's
requirement and the expected structure once, then one entry per finding (location,
message, fingerprint prefix of at least 7 characters), as one JSON document that
validates against the `CheckReport` schema. See "Compact output" in [agents.md](agents.md).
`--detail full` gives one self-contained record per finding, which an agent can act on
without another lookup. The hooks print the same findings as one text block per finding
for the agent to read:

```text
design/errors  info (heuristic)  src/lib.rs:2:7
  owner:       demo::run#function
  message:     .unwrap() can panic in run: return an error instead
  requirement: Panic MUST be limited to impossible programmer errors, `Must*`-style APIs, or documented hot-path invariants with one recovery boundary; normal runtime failures MUST return errors.
  context:     Panics bypass the caller's control flow.
  evidence:    function=run kind=unwrap
  expected:    valid rust example `rust-result` (src/lib.rs), canonical
                 pub fn parse(text: &str) -> Result<u32, std::num::ParseIntError> { ... }
  fingerprint: 395d1985afe8
  resolve:     lighthouse review resolve 395d1985afe8 --judgment <pass|fail|notApplicable> [--suppress <justification>] [--reason <text>] --attributed-to agent:<id>
```

The requirement and context come from the catalog. The expected structure is a valid
example of the decision for the file's language, at most 12 lines and 600 characters,
chosen in this order, and the block says why: the example marked `canonical` (at most
one per language and kind, enforced by the catalog validation); a valid example whose name
or whose invalid counterpart's name mentions the kind or visibility of the finding's
symbol; the shortest valid example. Evidence
that repeats the owner symbol is left out and long values are cut. Fingerprints are
shown as 12-character prefixes, longer when two shown findings would collide, and the
store accepts any unambiguous prefix. Only findings that ask for review carry a
resolve command; the choices to make are the judgment, its suppression and its reason, listed once after the
blocks (`judgments:`) and in the summary record of the JSON. A finding reported although a
judgment exists says why in a `note`. `--limit N` prints the N most severe findings (errors
first, original order kept) and a `... N more` tail or a `truncated` record; the summary
always counts everything. Both formats end with a summary, printed for clean runs too,
with the incomplete, suppressed and allowed counts, so silence is never read as success.

## Fixing

A rule judges, a fixer proposes, the orchestrator executes. Rules never edit, fixers
never apply, the orchestrator never judges. `check` stays pure: a fix is computed on
demand, when `check --fix` or the MCP `fix` tool asks.

**One canonical fix per rule.** The catalog is the single source that says how a rule is
fixed: the optional `fix:` block of its decision (see [Fixer spec](fixer-spec.md)). The
resolution is deterministic, `finding.decision` to `decision.fix` to the fixer registered
under the decision's id; there is no list of competing fixers and no fallback chain.
`--fixer <id>` is an explicit override that must name a registered fixer; it replaces the
decision's fixer for the selected findings, whose proposals count as `suggested`, and it
is the way to use a fixer for a rule that has no `fix:`. Documentation, `decision show` and the
skill show `fixable: safe|suggested`.

**Edit operations, not a round-trip IR.** Fixers do not rewrite source and there is no
lift-transform-lower pipeline (that would need a code generator per language and lose
comments and formatting). A fixer answers with `EditOp`s (`lighthouse_model::ucm`) over code model (UCM) nodes, and the
orchestrator lowers them to text edits with the spans providers report:

| Operation | Meaning | Needs |
| --- | --- | --- |
| `Move { node, anchor: Before\|After(node) }` | relocate a declaration with its extent | `extent` |
| `Delete { node }` | remove a declaration with its extent | `extent` |
| `Reorder { owner, order }` | put declarations in this order within the places they occupy | `extent` |
| `Rename { symbol, name }` | rename at the declaration and every reference site | `reference-sites`, every reference edge semantic |
| `DeleteRange { file, span }` | remove text, such as a comment | none |
| `Replace { file, span, text }` | replace text; an empty range inserts | none |

`Symbol.extent` is the whole declaration (doc comments, attributes and annotations
included; a Go spec of a parenthesized group has its own lines) and `Edge.site` is the
span of the identifier at a reference: both are optional UCM fields reported by
providers that declare the capabilities `extent` and `reference-sites`. A fixer whose
required capability the file's provider lacks is declined before it is asked: never a
guess. Moves work on whole lines. A moved block keeps its blank-line separation, and the
text edit that removes it takes one adjacent blank line. A scan of brackets (comments,
strings and characters skipped) tells which container a declaration sits in: a move
needs both ends in the same one (the same file level, the same `impl` or class body), a
`reorder` sorts each container on its own. A member of a parenthesized group (Go's
`const ( ... )`, whose order can carry meaning) is never moved alone: a `reorder` moves
the group whole, from its first line, with the comments that lead it, to its closing
paren, and declines the file when the order would split it (its members are not listed
together and in file order, or one is not listed). A declaration that shares its lines
with other code is not moved alone (a block comment that opens after the closing brace
counts as code). The scan reads comments and strings the way the file's language does
(nested block comments and raw strings are Rust's; a backtick quotes in Go; an unknown
language is read like C), which is a heuristic that the re-check backs up. Line endings
follow the file: the dominant one (LF or CRLF) is used where an edit inserts a line.

A `Rename` is refused unless the provider declares `complete-references` (every
reference, in signatures, fields and receivers too, is an edge with a site), the symbol is
private to its unit, the new name is an identifier that is no keyword and collides with no
sibling declaration, and every reference edge is semantic and has a site. No bundled
provider declares `complete-references` yet (Go lacks sites in signatures, fields and
receivers, and Rust edges are syntactic), so a rename is declined everywhere for now; a
catalog that uses `rename` must list `complete-references` in `requires`.

**Containment.** Every file an operation or a command result names must be relative and
normalized (no `..`, not absolute), a non-generated file of the analyzed project, and
reached through no symlink; anything else is declined before any text is read.

**Safety.** A proposal is `safe` or `suggested`. The decision's `safety` caps what its
fixer may claim: a fixer that returns `safe` for a `suggested` decision is downgraded.
`safe` is reserved for mechanical decisions (the catalog validation refuses it elsewhere).
By default only safe fixes of mechanical rules are applied; `--unsafe-fixes` (MCP
`unsafeFixes`) also applies suggested ones. Eligibility is decided from the decision before
a fixer is asked, so a suggested fix, a command included, never even runs without
`--unsafe-fixes`; the finding is reported as left alone, with the reason. `--fixer <id>`
(the `fixer` argument of the `fix` tool) names a registered fixer for the selected findings and
needs `--decision` or fingerprints.

**The orchestrator** (`lighthouse-engine`, the only component that writes) runs rounds,
at most five:

1. check the whole project with every enabled rule; the run refuses to start when the
   analysis is incomplete, because a fix that rests on a partial analysis is a guess;
2. select the findings (paths, rules, fingerprints; never those a judgment hides),
   ask each one's fixer for a proposal, lower it, and keep the proposals that do not
   collide (safe ones first, then report order; a proposal with exactly the edits of a
   kept one is covered by it);
3. apply the edits to the texts **in memory** (a proposal whose own edits overlap is
   declined; an out-of-range or overlapping edit is an error, never a panic);
4. run the language's formatter, `[languages.<id>] formatter`, which follows the command
   contract of fixes: exit `0` succeeds, stdout carries only the result, stderr is for
   people, the environment is cleared to an allowlist plus the declared `env`, and a
   timeout takes the process group down. Short form: `["gofmt", "-w"]` formats a scratch
   copy of the file (its path appended, or filling a `{file}` argument), `inPlace`. Table
   form: `{ argv = [...], stdin = "none|file", output = "inPlace|text", env = {...} }`;
   with `stdin = "file"` and `output = "text"` the formatter filters the text and no scratch
   file or module tree is needed (this repository uses `{ argv = ["rustfmt", "--edition",
   "2024", "--emit", "stdout"], stdin = "file", output = "text" }` and `gofmt` the same way).
   `{path}` is the file's real project-relative path, for tools like `--stdin-filepath`;
   `inPlace` formatters get the settings files found between the file and the root
   (`rustfmt.toml`, `.rustfmt.toml`, `.editorconfig`, `rust-toolchain.toml`) copied next to
   their scratch copy, and stdin formatters find them from the project root, their working
   directory. It runs only in a trusted project; otherwise it is skipped with a
   note and verification still runs. The key is the host's and never reaches the provider;
5. re-index and re-check **over overlays**: the engine reads the project with the
   candidate texts standing in for the files (protocol `context.overlays`; nothing is
   read from or written to those files). A changed file is dropped, back to its text
   before the round, when its check got worse: more findings of error or warn severity for
   some rule in the file (counted per file and decision, as a model fix's verdict is), a new
   gap in the analysis, or a failed formatter; so is every file of a fix that spans files.
   Dropped fixes are reported as declined.

It stops when a round applies nothing, after five rounds, or when a round reaches a state
an earlier one had (fixes that undo each other; the note names their rules).
**Nothing touches the disk until the last round passed.** Then the write has two phases.
First every changed file's hash is checked against the text the run analyzed (recorded when
it was read), and a fix that spans a file that changed is skipped as a whole, all its files,
before anything is written; those fixes are declined as changed concurrently. Then each file
is written atomically (permissions kept), containment re-checked and the hash checked again
right before the rename; if a write still fails, the files already written are put back and
nothing counts as applied. A `--dry-run` never writes. A panic or Ctrl-C before the write
leaves the disk as it was: there is no journal to restore. Declined fixes accumulate over
the rounds without duplicates. A fix is declined, before its fixer is asked, in a file whose
provider does not declare the `overlays` capability. Language plugins never verify fixes themselves:
verification is the re-check on the code model (UCM). An applied fix is not recorded: it records what happened to this checkout, not a decision.
The run that ends the fixing resolves the findings the fixes removed.

Bundled fixes: `design/declaration-groups` (a `reorder` by `design/group`, plus a `move`
for constructors) and `testing/file-layout` (a `move` of a fixture above, or a helper
below, the tests) are safe; `design/layout` and
`design/layout` (a `move` next to the related symbol or the caller),
`design/comments` and `core/no-unused-allow` (a `delete` of the comment or the
annotation line) are suggested. An `impl` block is not a symbol, so
reordering a Rust file cannot move one: such findings are declined and fixed by hand.
`lighthouse check .` on this repository is clean, and `check --fix` brought it there.

## Fixer spec

The `fix:` block of a decision selects one fixer kind that compiles into a provider of the
single `Fixer` interface. The kinds, their operations and their testing are in
[fixer-spec.md](fixer-spec.md).

## Crates

Dependencies point down only, by the layer table in `lighthouse.toml`; the project configuration (`Project`, rule levels, overrides) is `spec::project`. Each crate's role, entry points and invariants are in its README.

| Layer | Crates |
|-------|--------|
| L5 | [cli](../crates/lighthouse-cli/README.md) |
| L4 | [session](../crates/lighthouse-session/README.md) |
| L3 | [report](../crates/lighthouse-report/README.md), [store](../crates/lighthouse-store/README.md), [ml](../crates/lighthouse-ml/README.md) |
| L2 | [engine](../crates/lighthouse-engine/README.md), [checks](../crates/lighthouse-checks/README.md), [rpc](../crates/lighthouse-rpc/README.md) |
| L1 | [plugin](../crates/lighthouse-plugin/README.md), [spec](../crates/lighthouse-spec/README.md) |
| L0 | [model](../crates/lighthouse-model/README.md), [resource](../crates/lighthouse-resource/README.md), `process`, all on [protocol](../crates/lighthouse-protocol/README.md), which depends on no workspace crate |

- [cli](../crates/lighthouse-cli/README.md): `surface`, `cli` and `mcp` frontends.
- [session](../crates/lighthouse-session/README.md): the shared operations; composition root.
- [report](../crates/lighthouse-report/README.md): output formats.
- [store](../crates/lighthouse-store/README.md): findings, judgments, the decision log.
- [ml](../crates/lighthouse-ml/README.md): model evaluation and training.
- [engine](../crates/lighthouse-engine/README.md): analysis and fix orchestration.
- [checks](../crates/lighthouse-checks/README.md): declarative checks, metrics, order keys, the bundled registry.
- [rpc](../crates/lighthouse-rpc/README.md): language plugins as processes.
- [plugin](../crates/lighthouse-plugin/README.md): SPI and Registry.
- [spec](../crates/lighthouse-spec/README.md): decision kinds, Catalog, local layer.
- [model](../crates/lighthouse-model/README.md), [resource](../crates/lighthouse-resource/README.md), [protocol](../crates/lighthouse-protocol/README.md): L0 vocabulary, resources and the wire; protocol also owns the hashes and folds that host and plugin must compute alike.
- `process`: child-process helper (no README). `test-support`: dev-only helper of the test suites (no README).
- Plugins: [lang-rust](../plugins/lang-rust/README.md), [lang-go](../plugins/lang-go/README.md), [model-http](../plugins/model-http/README.md), [fake-model](../plugins/fake-model/README.md).

Each crate has one integration-test binary, `tests/it/main.rs` with a module per area.

The core never knows the code model (UCM): the modules of `model` other than `ucm`, and the `store`, `ml`, `report` and `session` crates, import nothing of `lighthouse_model::ucm`. A `forbidden` contract of `design/layers` in `lighthouse.toml` enforces it.

## Models

A model is the provider of a task, named `<provider>/<model>`, and a plugin kind beside language
providers. A task has one standard body: `classification` (TypeSafe System One), `embedding`
(OpenAI Embeddings) and `edit` (an agent session in a scratch copy). Chat Completions are not a task:
they are the logprobs adapter behind `classification`. Lighthouse generates nothing itself; a
model fix is an agent's `edit`. The probability of a
classification comes from the model's own distribution, never from a number the model writes.

A project binds `classification` in layers chosen per decision, where an abstention falls through
to the next (the attached Revision, `rules."<id>".classification`, `[models] classification`, then a review
task); a layer may be a route of equivalent models that fails over on error or rotates. The
runtime, the wire, `model-http`, labels, datasets, `decision eval` and `model train` are in
[models.md](models.md).

## Frontends

The CLI, the MCP server and the agent hooks are frontends over one shared layer,
`lighthouse-session`: it loads a project (configuration, local rules, catalog),
runs a check and remembers it, records judgments, tests and authors rules, and
renders the agent skill. A frontend parses its own input and prints the result;
it holds no logic of its own, so a judgment recorded through MCP, the CLI or a
hook is the same record in the same log. Fixing is the same: `check --fix [--dry-run]
[--unsafe-fixes] [--fixer <id>]` and the MCP `fix` tool call one session operation.
Hooks never fix.

```
 lighthouse-cli ── surface: the operations both frontends share (check [--fix], review, decision, ...)
                ├ cli: the terminal (docs, init --agent, hook claude-code, ...)
                └ mcp: `lighthouse mcp`, tools and resources over stdio (rmcp)
        └── lighthouse-session ── engine, ml, store, spec, checks, rpc, report
```

Rule authoring is gated: `decision_create` and `decision_update` build the candidate
local layer in memory, compile its rules and run every example through the whole
engine; the file under `.lighthouse/decisions` is written (through a temporary name)
only when all of that passes, so a rejection leaves the project untouched.

The agent skill is generated from the catalog and the configuration
(`lighthouse docs generate` writes `skills/lighthouse/SKILL.md`, `docs check`
covers it; `init --agent claude-code` writes the project's own copy): the loop,
rules of conduct, how to query rules, and a digest of the active decisions.
See [agents.md](agents.md).

## Dogfooding

Lighthouse checks its own sources. The repository's `lighthouse.toml` enables the
bundled `core`, `design` and `testing` plugins with their recommended projects (`<plugin>/strict` adds the advice that is too noisy to recommend, such as `design/errors`) and
the Go and Rust plugins built by `make plugins`, and `make lint` (part of `make ci`)
ends with `lighthouse check .`, which must exit 0: a finding is either fixed in the code, or it exposes a rule or
provider that is imprecise, and that is fixed instead of silenced. The only
overrides are for `crates/lighthouse-rpc/tests/conformance/**`, whose fixtures describe code on
purpose.

The Rust plugin resolves names syntactically, so its edges are a lower bound
(see the `lang-rust` section of [plugin-protocol.md](plugin-protocol.md)). Rules
that count callers therefore have to fail safe: a method called through a
receiver whose type is not written in the code is recorded as a possible use of
every non-`pub` method of that name (a `heuristic` edge that counting analyses
ignore), which keeps `design/symbols` from
reporting a method that has an unseen caller.
