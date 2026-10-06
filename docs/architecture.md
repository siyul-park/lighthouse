# Architecture

```
 CLI ─ engine ─ config ─ reporters ─ store (.lighthouse/lighthouse.db)
          │
   plugin registry ── in-process plugins: core, metrics, design, testing,
          │            and `local` (declarative rules of the project)
          │
   RPC host (lighthouse-rpc) ── language plugins as processes
          │                      plugins/lang-go (go/packages + go/types)
          │                      plugins/lang-rust (syn + own module resolution)
   unified code model (lighthouse-model)
```

The core knows no language and no rule. Languages arrive as providers that turn
files into the unified code model (symbols, edges, function summaries, tests);
analyzers and rules read only that model. In-process providers and out-of-process
plugins (see [plugin-protocol.md](plugin-protocol.md)) implement the same
`LanguageProvider` contract, which is project-level: the engine calls each
provider once with all of its files.

Dependency direction runs from the general to the specific:
`model` ← `plugin` (contracts) ← `protocol` (wire types) ← `rpc` (host and the only
wire-to-model conversion) ← `engine` ← `cli`. The wire model is separate from the
core model: renaming a core field is not a protocol change. `store` knows only
`model` (artifact-neutral records), and `report` knows `model` and the pattern
catalog in `spec`.

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

## Rules

Every implemented rule belongs to a catalog pattern whose `implementation` is
either `builtin` (a Rust rule of a bundled plugin: `design`, `testing`, `core`) or
`declarative`: a YAML file with a CEL expression over the code model.

```yaml
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
`kind id module name`); modules `path name test_of files symbols`; files `path
lang test generated lines symbols functions`; tests add `nesting style targets
target_count`. Absent values are empty strings, never null. `edge` and `module`
rules run once over the project, the others once per file. Projects add their
own rules as `.lighthouse/rules/*.yaml` (a pattern with id `local/<name>` and
the rule file under `rule:`), enabled through the `local` plugin of
`lighthouse.toml`. `lighthouse rule test [ids]` runs every example of the
bundled and local patterns through the whole engine, in each language whose
plugin the configuration lists.

Language specifics stay out of the rules: provider facts (symbols, owners,
visibility, spans, comments) are the same everywhere, and per-language option
defaults and tuning prose in the pattern realize a rule per language.

## Incomplete analysis

`Outcome.incomplete` is first-class: "not checked" never equals "passed".

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

## Memory: findings, verdicts and the feedback loop

`lighthouse check` remembers what it saw, in `.lighthouse/lighthouse.db` at the
project root (SQLite, the `lighthouse-store` crate). The file is a local cache of one
machine's checks and reviews, so `lighthouse init` adds `.lighthouse/*.db*` to
`.gitignore`. A run is recorded by default whenever the project has a
`lighthouse.toml`; `--no-store` runs without reading or writing the store. A store that
cannot be opened never fails a check: the run goes on without memory and says so on
stderr. The schema is versioned with `PRAGMA user_version` and migrated on open; a
database written by a newer build is refused.

The store is artifact-neutral: a finding's artifact is a path string and its locator is
JSON (`{"span": {...}}` for text, a JSON pointer or region id for other artifacts), and
evidence and facts are JSON, so nothing in it is specific to code.

**`findings`** has one row per fingerprint: rule, severity, path, locator, owner
symbol, `first_seen`, `last_seen`, `resolved_at`, how often it came back
(`reopened`), and the last message, evidence and facts. The facts are what the
analysis knew about the finding's subject in its last run: language, symbol kind,
visibility, callers and callees, the function summary, and the measures analyzers took
(cognitive, cyclomatic, fan-in and fan-out, size). They are kept per open finding so a
review can snapshot them without re-analyzing.

A run updates `findings` like this:

- every reported finding is inserted or refreshed, a resolved one is reopened;
- a finding that was open inside the run's *report scope* and rules and is absent now
  is marked resolved;
- nothing is resolved outside the report scope (`check src/a.rs`, `--changed`, `--diff`
  narrow it), for rules that did not run (`--rules`), or when the analysis was
  incomplete. "Not checked" never means "fixed".

A fingerprint is the rule, the owner symbol's path (`module::owner::name#kind`) or the
file for file-level rules, and a whitespace-normalized snippet where the finding has
one. Line numbers are not part of it, so edits above a finding do not change its
identity. Findings that repeat within a file get an occurrence index, so removing the
first of several identical findings renames the others.

**`review_events`** is the append-only log of verdicts (triggers reject `UPDATE` and
`DELETE`). Each row carries the finding's fingerprint, rule id, `rule_version` (a hash of
the pattern definition), `catalog_version` (a hash of all patterns), a nullable
`pattern_fingerprint` (code-pattern identity, filled by the similarity index), the
verdict `confirmed | rejected | deferred`, the reason code, free text, the reviewer
(`agent | human` and id), language, scope, evidence, a `feature_snapshot` frozen at review
time (the finding with its evidence and facts), the git commit and a timestamp.

| Verdict | Reasons | Effect |
| --- | --- | --- |
| `confirmed` | `fixed`, `accepted-debt`, or none | the finding is right; stays visible |
| `rejected` | `false-positive`, `intentional-exception`, `scope-too-broad`, `project-allowed`, `not-worth-fixing` (required) | suppressed |
| `deferred` | none | stays visible, marked deferred in `review list` |

**Suppression** is derived, not stored: the latest verdict per fingerprint is the finding's
standing (views `latest_verdicts` and `suppressions`). A fingerprint whose latest verdict is
`rejected` is left out of reports and out of the exit code, and `check` prints
`N finding(s) suppressed by review verdicts`; `review list --status suppressed`
lists them. `scope-too-broad` also suppresses and is flagged (`narrowing`) as a hint
to narrow the rule. A later `confirmed` or `deferred` verdict lifts the suppression.
Suppression is by fingerprint, so it follows the symbol across edits and ends when the
symbol is renamed or moved.

**Labels** are what a verdict teaches later models about its rule: `confirmed` is
positive; `rejected` as `false-positive` or `scope-too-broad` is negative; the other
rejections (`intentional-exception`, `project-allowed`, `not-worth-fixing`) are
separate targets that say nothing about precision; `deferred` is unlabeled. A finding
nobody reviewed has no label and is never a negative.

### Agent output

`check --format agent` prints one block per finding that an agent can act on without
another lookup, `--format agent-json` the same records as JSON lines (`finding`,
`incomplete` and `summary` records tagged by `type`):

```text
design/private-helper-callers  review (heuristic)  src/lib.rs:5:1
  owner:       demo::clamp#function
  message:     private function clamp has one caller (run); review whether ...
  requirement: A private helper SHOULD have at least two callers.
  intent:      A private helper with one caller is usually part of that caller.
  evidence:    caller=demo::run#function callers=1 statements=3
  expected:    valid rust example `rust-valid` (src/lib.rs)
                 pub fn run(x: u8) -> u8 { ... }
  fingerprint: 395d1985...
  resolve:     lighthouse review resolve 395d1985... --verdict <verdict> --reason <reason> --reviewer-kind agent
```

The requirement and intent come from the catalog; the expected structure is the first
valid example of the pattern for the file's language (at most 12 lines and 600
characters), or the pattern's tuning note for that language, or a valid example of any
language, in that order. Only findings at the `review` severity carry a resolve
command; the choices to make are the verdict and the reason, listed once after the
blocks (`reasons:`) and as a table in the JSON record. Both formats end with a
summary that is printed for clean runs too, with the incomplete and suppressed
counts, so silence is never read as success.

## Dogfooding

Lighthouse checks its own sources. The repository's `lighthouse.toml` enables the
bundled `core`, `design` and `testing` plugins with their recommended presets (`<plugin>/strict` adds the review-level
advice that is too noisy to recommend, such as `design/private-helper-callers`) and
the Go and Rust plugins built by `make plugins`, and `make lint` (part of `make ci`)
ends with `lighthouse check .`, which must exit 0: a finding is either fixed in the code, or it exposes a rule or
provider that is imprecise, and that is fixed instead of silenced. The only
overrides are for `plugins/conformance/**`, whose fixtures describe code on
purpose.

The Rust plugin resolves names syntactically, so its edges are a lower bound
(see the `lang-rust` section of [plugin-protocol.md](plugin-protocol.md)). Rules
that count callers therefore have to fail safe: a method called through a
receiver whose type is not written in the code is recorded as a possible use of
every non-`pub` method of that name (a `heuristic` edge that counting analyses
ignore), which keeps `design/single-use-wrapper` from
reporting a method that has an unseen caller.
