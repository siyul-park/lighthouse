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

Memory has two parts with different owners. **Sightings** (what `check` saw, when, with
which facts) stay on one machine, in `.lighthouse/lighthouse.db` (SQLite, the
`lighthouse-store` crate). **Decisions** (verdicts on findings) are shared through a
committed file, `.lighthouse/decisions.jsonl`, and the database holds a copy of them
that it rebuilds from the file. See [Shared decisions](#shared-decisions).

`lighthouse init` ignores `.lighthouse/*.db*` and marks the decision log `merge=union`
in `.gitattributes`. A run is recorded by default whenever the project has a
`lighthouse.toml`; `--no-store` runs without reading or writing the store and applies no
verdicts. A store that cannot be used never fails a check: the run goes on, says so on
stderr, and still applies the verdicts it can read. The database uses WAL with
`synchronous=NORMAL`, writers take the write lock up front (`BEGIN IMMEDIATE`) and wait
for each other, and opening and migrating are serialized the same way, so a hook, a
stop script and a human can run at once. The schema is versioned with
`PRAGMA user_version`; a database written by a newer build is refused, and a file that
is not a database is named in the error and left where it is (move it aside; it is
rebuilt from the log, and only the history of sightings is lost).

The store is artifact-neutral: a finding's artifact is a path string and its locator is
JSON (`{"span": {...}}` for text, a JSON pointer or region id for other artifacts), and
evidence, facts and options are JSON, so nothing in it is specific to code.

### Findings

`findings` has one row per fingerprint: rule, last severity and tier, path, locator,
owner symbol, `first_seen`, `last_seen`, `resolved_at`, `inactive_at`, how often it came
back (`reopened`), and the last sighting: message, evidence, facts, the options the rule
ran with, the commit and whether the tracked files were dirty, the Lighthouse and
catalog versions, the semantic version of the rule and a digest of the evidence. The
facts are what the analysis knew about the subject: language; for a symbol its kind,
visibility, owner, callers (split into same-module and other-module), callees, function
summary and the measures analyzers took (cognitive, cyclomatic, fan-in and fan-out,
size); for any other finding the file's size and the measures taken of the file. They are
kept per finding so a review can snapshot them without re-analyzing.

A run updates `findings` like this:

- every reported finding is inserted or refreshed, a resolved one is reopened;
- a finding that was open inside the run's *report scope* and rules and is absent now is
  marked resolved. `--changed` and `--diff` include deleted files in the scope, so what a
  deleted file had is resolved;
- nothing is resolved outside the report scope (`check src/a.rs`, `--changed`, `--diff`
  narrow it), for rules that did not run (`--rules`), or under a directory that holds a
  file the run could not analyze. A gap that cannot be placed (a language provider that
  failed) stops all resolving. "Not checked" never means "fixed";
- an open finding of a rule the configuration no longer enables becomes `inactive`: it
  is not open, not resolved, and listed with `review list --status inactive`. It is
  reactivated if the rule is enabled again.

`review prune [--older-than DAYS]` deletes resolved and inactive findings that nobody
reviewed; findings with verdicts stay, because the verdicts are labels.

### Fingerprints

A fingerprint is the rule, the owner symbol's path (`module::owner::name#kind`) or the
file for file-level rules, and a whitespace-normalized snippet where the finding has
one. Line numbers are not part of it, so edits above a finding do not change its
identity. When findings of one rule in one file still share a fingerprint, the engine
tells them apart by the finding's symbol, or the symbol that encloses or precedes it,
and only the ones that still collide get an ordinal. Identities that rest on an ordinal
are marked in the facts (`ordinal`), and `review resolve` warns about them: the ordinal
moves when an identical finding appears before it. A finding that stops colliding goes
back to its undistinguished fingerprint.

### Verdicts

A verdict is a review event: the finding's fingerprint, rule id, `rule_version` (the
*semantic* version of the pattern: a hash of its requirement, enforcement, options and
implementation, not of prose, examples or tuning notes), `pattern_hash` (the whole
pattern definition), `catalog_version`, the Lighthouse version, a nullable
`pattern_fingerprint` (code-pattern identity, filled by the similarity index), the
verdict, the reason, free text, the reviewer (`agent | human` and id), language, scope,
a digest of the evidence, a **snapshot** frozen at review time, the commit and a
timestamp. The snapshot is one JSON object, versioned (`"v": 1`): message, path,
locator, symbol, evidence, facts, options, severity, tier, when the finding was seen
(`seen_at`), the commit and dirtiness at that sighting, the Lighthouse version and
`pattern_fingerprint: null`. The snapshot is the single owner of the frozen evidence.

| Verdict | Reasons | Effect |
| --- | --- | --- |
| `confirmed` | `fixed`, `accepted-debt`, or none | the finding is right; stays visible |
| `rejected` | `false-positive`, `intentional-exception`, `scope-too-broad`, `project-allowed`, `not-worth-fixing` (required) | suppressed while valid |
| `deferred` | none | stays visible, marked deferred in `review list` |

The reviewer is `--reviewer-kind` / `--reviewer-id`, else `LIGHTHOUSE_REVIEWER_KIND` /
`LIGHTHOUSE_REVIEWER`, else `human` and `$USER`. Tools that run reviews for an agent
(hooks, an MCP server) set both variables, so `agent` is recorded without the agent
having to remember it. `review resolve --seen <last seen>` refuses the verdict when the
finding has been seen again since the reviewer read it, and the finding is read in the
same transaction that records the verdict. `review resolve` works without a readable
catalog: it warns, and the versions are left out.

**Suppression is derived**, never stored, from the latest verdict per finding (ordered by
time, then event id). A rejected verdict suppresses only while it is valid:

- the finding is not mechanical: findings of severity `error` are decided by the rule, so a
  verdict on one is recorded but never suppresses it, and `check` says so (`rejected as
  <reason> - mechanical findings are not suppressible; fix the rule`);
- the semantic version of the rule is the one the verdict judged (otherwise `verdict
  expired: rule changed`);
- the digest of the finding's normalized evidence is the one the verdict judged (otherwise
  `verdict expired: evidence changed`).

An expired verdict leaves the finding in the report with a note, so it is asked again. A
verdict from before a version or digest was recorded matches anything. `scope-too-broad`
also suppresses and is flagged as a hint to narrow the rule: `review list --status
narrowing` lists those. `check` prints how many findings verdicts suppressed.

**Labels** are what a verdict teaches later models about its rule: `confirmed` is
positive; `rejected` as `false-positive` or `scope-too-broad` is negative; the other
rejections (`intentional-exception`, `project-allowed`, `not-worth-fixing`) are separate
targets that say nothing about precision; `deferred` is unlabeled. A finding nobody
reviewed has no label and is never a negative.

## Shared decisions

Three things decide that a finding is not reported, with different lifetimes:

| | Lives in | Shared by | Holds while |
| --- | --- | --- | --- |
| Verdict | `.lighthouse/decisions.jsonl` (committed) | git | the rule's semantic version and the finding's evidence are unchanged; never for mechanical findings |
| Source annotation | a comment in the code | git, in the diff | the comment is there; reported when it suppresses nothing |
| Sighting history | `.lighthouse/lighthouse.db` (ignored) | nobody | it is a local cache |

**The decision log** is the source of truth for verdicts. `review resolve` appends one
JSON object per verdict (canonical, compact, keys sorted, so diffs are one line) with a
single write and `fsync`, then updates the cache. Every entry's `id` is a hash of the
entry without it. Opening the store imports the entries the cache does not have and
ignores the ones it has, so the cache can always be deleted and rebuilt, a teammate's
verdicts apply as soon as the log is pulled, and CI suppresses what developers
suppressed. Two branches that both appended merge by keeping both lines
(`.gitattributes`: `merge=union`); the same entry twice is one entry; ordering uses the
entries' timestamps. A line that is not an entry, whose id does not match its content, or
whose verdict and reason do not belong together stops the store with the line number, so a
decision is never skipped silently. A verdict recorded in a cache from before the log
existed is exported to the log once. Verdicts are never edited or deleted; a later
verdict replaces the standing of the finding.

**Source annotations** put the decision next to the code it is about, where a reviewer
sees it change. A comment line that starts with `lighthouse:allow <rule>[, <rule>] --
<reason>` allows the named rules on the symbol it documents, on the line after it and on
its own line. It applies at every severity, mechanical errors included, because it is
reviewed in the diff, and the report counts what was allowed (`N allowed`). The reason is
required: an annotation without one is ignored and reported by the mechanical rule
`core/annotation-reason`. An annotation whose rule no longer fires there, or is not
enabled, is reported by `core/unused-allow` so annotations do not rot (when
`--rules` leaves the annotated rule out of the run, nothing is said). Both rules belong to
the `core` pack, so `core/recommended` enables them; the engine evaluates them because
whether an annotation is used depends on every other rule's findings. Prose that merely
mentions the marker mid-line is not an annotation.

### Agent output

`check --format agent` prints one block per finding that an agent can act on without
another lookup, `--format agent-json` the same records as JSON lines (`finding`,
`incomplete`, `truncated` and `summary` records tagged by `type`):

```text
design/private-helper-callers  review (heuristic)  src/lib.rs:5:1
  owner:       demo::clamp#function
  message:     private function clamp has one caller (run); review whether ...
  requirement: A private helper SHOULD have at least two callers.
  intent:      A private helper with one caller is usually part of that caller.
  evidence:    caller=demo::run#function callers=1 statements=3
  expected:    valid rust example `rust-valid` (src/lib.rs), canonical
                 pub fn run(x: u8) -> u8 { ... }
  fingerprint: 395d1985afe8
  resolve:     lighthouse review resolve 395d1985afe8 --verdict <verdict> --reason <reason> --reviewer-kind agent
```

The requirement and intent come from the catalog. The expected structure is a valid
example of the pattern for the file's language, at most 12 lines and 600 characters,
chosen in this order, and the block says why: the example marked `canonical` (at most
one per language and kind, enforced by the catalog validation); a valid example whose name
or whose invalid counterpart's name mentions the kind or visibility of the finding's
symbol; the shortest valid example; the pattern's tuning note for the language. Evidence
that repeats the owner symbol is left out and long values are cut. Fingerprints are
shown as 12-character prefixes, longer when two shown findings would collide, and the
store accepts any unambiguous prefix. Only findings at the `review` severity carry a
resolve command; the choices to make are the verdict and the reason, listed once after the
blocks (`reasons:`) and in the summary record of the JSON. A finding reported although a
verdict exists says why in a `note`. `--limit N` prints the N most severe findings (errors
first, original order kept) and a `... N more` tail or a `truncated` record; the summary
always counts everything. Both formats end with a summary, printed for clean runs too,
with the incomplete, suppressed and allowed counts, so silence is never read as success.

## Frontends

The CLI, the MCP server and the agent hooks are frontends over one shared layer,
`lighthouse-session`: it loads a project (configuration, local rules, catalog),
runs a check and remembers it, records verdicts, tests and authors rules, and
renders the agent skill. A frontend parses its own input and prints the result;
it holds no logic of its own, so a verdict recorded through MCP, the CLI or a
hook is the same event in the same log.

```
 lighthouse-cli ── check, review, rule, docs, init --agent, hook claude-code
 lighthouse-mcp ── `lighthouse mcp`: tools and resources over stdio (rmcp)
        └── lighthouse-session ── engine, store, spec, declarative, rpc, report
```

Rule authoring is gated: `rule_create` and `rule_update` build the candidate
local layer in memory, compile its rules and run every example through the whole
engine; the file under `.lighthouse/rules` is written (through a temporary name)
only when all of that passes, so a rejection leaves the project untouched.

The agent skill is generated from the catalog and the configuration
(`lighthouse docs generate` writes `skills/lighthouse/SKILL.md`, `docs check`
covers it; `init --agent claude-code` writes the project's own copy): the loop,
rules of conduct, how to query rules, and a digest of the active patterns.
See [agents.md](agents.md).

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
