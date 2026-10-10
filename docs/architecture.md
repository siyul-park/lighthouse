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
   unified code model (lighthouse-model)
```

The core knows no language and no rule. Languages arrive as providers that turn
files into the unified code model (symbols, edges, function summaries, tests);
analyzers and rules read only that model. In-process providers and out-of-process
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
| `Plugin` | `PluginManifest { id, version }` | `languages`, `analyzers`, `rules`, `presets`, `fixers`, `order_keys` |
| `LanguageProvider` | `ProviderManifest { id, globs, conventions, capabilities, fallback, priority }` | `index` |
| `Analyzer` | `AnalyzerManifest { id, requires, scope }` | `run` |
| `Rule` | `RuleManifest { id, severity, scope, description, docs, analyzers, capabilities, citation, strict }` | `validate`, `check` |
| `Preset` | `PresetManifest { id, rules }` (pure data) | none |
| `Fixer` | `FixerManifest { id, requires }` | `fix` |
| `OrderKey` | `OrderKeyManifest { id, description }` | `rank` |

The `initialize` result of a plugin process is its `PluginManifest` plus one
`ProviderManifest` per language, field for field (see
[plugin-protocol.md](plugin-protocol.md)).

Dependency direction runs from the general to the specific:
`model` ← `plugin` (contracts) ← `protocol` (wire types) ← `rpc` (host and the only
wire-to-model conversion) ← `engine` ← `cli`. The wire model is separate from the
core model: renaming a core field is not a protocol change. `store` knows only
`model` (artifact-neutral records), and `report` knows `model` and the decision
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

## Terms

| Term | Means |
| --- | --- |
| **decision** | What was decided and why: the catalog entry (`Decision`), with its severity, check, options, examples and fix. The one authored concept. |
| **rule** | The executable form the engine compiles from a decision's `check`: the `Rule` trait, the rule ids that findings carry (a rule id is the id of its decision). |
| **verdict** | A judgment about one finding (confirmed, rejected, deferred, with a reason), appended to the decision log. |
| **annotation** | A verdict written at the code: `lighthouse:allow <rule> -- <reason>`. |

## Resource model

Every spec document has one envelope, in the style of the Kubernetes resource
model, and is read from YAML, TOML or JSON alike (a YAML file may hold several
documents):

```yaml
apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: core/max-file-lines        # identity, `<namespace>/<kebab-name>`
  labels: { lighthouse/pack: core, lighthouse/section: limits }
  annotations: {}
spec: { ... }
```

| Kind | Holds | Lives in |
| --- | --- | --- |
| `Decision` | a decision: `title intent scope requirement status supersedes consequences severity evidence exceptions options languages check fix citation strict examples` | `decisions/<pack>/<section>/<name>.yaml`, `.lighthouse/decisions/` |
| `DecisionOverride` | a project's adjustment of a decision of a lower layer (`extends`) | `.lighthouse/decisions/` |
| `Pack` | title, intro and the ordered sections, each listing its decisions | `decisions/<pack>/pack.yaml` |
| `SourceMap` | which decision covers each normative line of the documents a catalog came from | `decisions/sources.yaml` |
| `Project` | `lighthouse.toml`: `plugins extends languages rules overrides` | project root |
| `Preset` | named rule configuration (`extends`, `rules`); the `recommended` and `strict` presets of a plugin are derived from its decisions | in code |
| `Plugin` | `lighthouse-plugin.toml`: `runtime {command, args}` and `provides` | plugin directory |
| `OrderKey` | a way to order declarations for a `reorder` fix | in code |
| `Verdict` | a record of the decision log | `.lighthouse/decisions.jsonl` |

Conventions: keys are lowerCamelCase (option names inside a decision stay as the
decision authors them), durations are strings (`30s`), enums are lowercase kebab,
paths are relative with `/`. A decision belongs to a pack and section by its
labels; the `Pack` document only orders them, so where the file lies carries no
meaning. `scope` is `{domain, subject}` (`domain` is `code`). `options` is a JSON
Schema object (`type`, `default`, `description` per property, closed with
`additionalProperties: false`); `languages.<id>` holds the option values and the
wording of one language. `check` and `fix` choose a provider by `type`: `check` is
`builtin`, `cel`, `command`, `rpc` or `model`; `fix` is `ops`, `command` or `rpc`, with the fields of its type
next to `safety` and `requires`, and no others.

The JSON Schema (2020-12) of each kind is generated from the Rust types into
`schema/` and checked in (`lighthouse schema [kind]` prints one, `--write DIR`
writes all; a test fails when the files are stale). Documents carry a
`# yaml-language-server: $schema=...` comment so editors validate them.
`lighthouse spec validate [paths]` checks each document against its schema and
against the others (presets and rules name things that exist, fix operations name
registered order keys, CEL compiles, examples are well formed; `--examples` also
runs them). `lighthouse spec migrate [paths]` rewrites the formats from before the
model (decision files of the earlier format, `pack.yaml` with `section.yaml`, rule files, `sources.yaml`,
`lighthouse.toml`, `lighthouse-plugin.toml`, `.lighthouse/rules`) and leaves a
document that already has an `apiVersion` alone, so it is idempotent. Readers
accept the new format only, and say so: a project that still has `.lighthouse/rules`,
a catalog of the earlier format or a configuration without an envelope fails to load
with a pointer to `lighthouse spec migrate`. TOML comments are not preserved by the
migration.

The migration touches only shapes it recognizes: a decision has an `id` and a
`requirement`, an override an `extends` and an override key, a configuration one of the
keys of the old one; a key it does not know is an error rather than dropped. A directory
scan covers the catalogs it finds (a directory with a `pack.yaml`), `.lighthouse/` and
the configuration files; other YAML or JSON (an `.eslintrc.yml`) is not read. A file
of several documents with one in the old format is refused, to be split first. Changes
are applied together: every target is written next to its destination and renamed into
place, and only then are files removed, so a failure never leaves a rule removed
before the decision that inlines it exists. A rule file or `section.yaml` is removed
only when a migrated document consumed it (a decision names a rule file relative to
the catalog root, or its own directory); otherwise it stays, with a warning.

## Severity

A finding is `error`, `warn` or `info` (and a rule may be `off` in
configuration). A decision authors its severity (`severity: error|warn|info`); a decision without a `check` is documentation and yields no finding. A project's `level` changes what is reported and the exit code, never the authored severity. The same
three levels map to SARIF (`error`, `warning`, `note`). Exit code: `1` if any error;
a warning fails only with `--strict` or past `--max-warnings N`; `info` never
fails; an incomplete analysis is `3`.

Needing a verdict follows the *authored* severity: `error` is definitive (no verdict, only an annotation in the code waives it, its fix may be `safe`); `warn` and `info` are review tasks whatever level the configuration reports them at.
`review list`, the MCP `review_tasks` tool, the agent format and the hooks select by
that property. A rule that has no decision is treated by its severity. History keeps
the severity string it recorded, including `review` from older builds.

Suppression follows the same property, not the severity: a verdict can suppress a
finding of a heuristic or judgment decision even when the project configured it as
`error`, and never one of a mechanical decision even when configured as `warn`. So an
agent is only asked for a verdict that can take effect. (A cache row from before authored severities were recorded has none; its recorded
severity stands in for it. Schema migration 6 turned the recorded tiers into
authored severities: mechanical is `error`, heuristic `warn`, judgment `info`.)

## Rules

Every implemented rule belongs to a catalog decision whose `check` is
one provider of the union `builtin | cel | command | rpc | model`, with the common fields `requires` and `timeout`:

- `builtin` is a standard, decision-agnostic operation (`order`, `proximity`, `cycle`) or, for the two rules the engine reports itself (`core/annotation-reason`, `core/unused-allow`), a rule registered by name.
- `cel` is an expression over the code model, written in the decision, with the standard function library `metrics callers callees edges owner tests annotations rank exposed` and the text helpers `lines trim trimPrefixes trimSuffixes trimLeft trimRight leadingRun drop`.
- `command` runs a program under the process contract: argv without a shell, `{file}`/`{files}`/`{rule}` filling whole arguments, `batch: file|all`, exit `0` clean, `1` findings (stdout lines, an optional `path:line[:col]: ` prefix), anything else an execution error that leaves the analysis incomplete (exit 3). It runs only in a project the user trusts (`lighthouse trust`), and the trust covers the program and every argument that names a file inside the project, by content, so `sh script.sh` is bound to the script. A command is not sandboxed: it runs with the user's privileges, in the project root, with an environment cleared to `PATH`, `LANG`, `TMPDIR` and the declared `env` (no `HOME`) plus `LIGHTHOUSE_*`. Trust is the only protection; a command that exits with an execution error, times out or prints more than the output cap leaves the analysis incomplete, never clean.
- `rpc` is reserved until plugin protocol 0.2 and refused at load.
- `model` hands the decision to agent review (`select` and `prompt` optional); it is not deterministic and caps the severity at `warn`.

A decision with no `check` is documentation. How a decision is checked is not part of what it means: the *meaning version* hashes requirement, severity, scope and options, and the `check_revision` records the check on findings and verdicts without ever expiring one.

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
`kind id module name`); modules `path name test_of files symbols`; files `path
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
defaults and tuning prose in the decision realize a rule per language.

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

## Speed and determinism

A run does the work that is independent in parallel and the rest once. Language
providers index side by side (`lang-go` also loads its nested modules
concurrently); files are checked in parallel, each by every rule that applies to
it, and project-scope rules in parallel with them. What each step returns is
gathered in a fixed order (files in path order, rules in id order), so findings,
fingerprints and exit codes are the same on every run whatever the schedule;
identity and fingerprint assignment run after the findings are collected and
ordered. What several rules would compute alike is computed once per run and
shared through `Ctx::memo`: the base facts of a symbol that CEL checks read, and
the versions of a decision that the store records with each finding. Set
`RAYON_NUM_THREADS=1` to run on one worker.

`lighthouse check --timings` prints where a run's time went to stderr, one
`timings:` line per phase: setup, read, each provider's index, merge, each
analyzer, the rules (wall clock, then the five slowest rules summed over the
files they ran on), identity, store, report and the total.

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

`findings` has one row per fingerprint: rule, last severity and authored severity, path, locator,
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

### Semantic version

`rule_version` is the *meaning version*: the hash of a decision's normalized semantic content
(requirement with whitespace squashed, severity, scope, and the option
types and defaults including each language's values; never the `check`): the envelope, the labels, the
file format and the prose do not change it. Hashing a different set of fields gives a
different number than builds before the resource model recorded, so a verdict
written then would expire for the change of format alone. Each finding therefore
also carries the *earlier* versions: what the build with `enforcement` and what the
build before the resource model gave for the same content
(`Decision::earlier_versions`, which reads the old shapes back out of the new; a
decision that moved from a bespoke rule to a standard operation remembers its old
id in `lighthouse/was-builtin`, one whose `enforcement` is not its severity's default
in `lighthouse/was-enforcement`). The store treats a verdict as current when its
`rule_version` equals the meaning version or any of them, so
existing verdicts keep applying while the decision demands the same, and expire as
before when it changes (one exception: `scope` was not hashed before, so changing
it expires only verdicts recorded after the model). A test pins the legacy value of
every bundled decision to the number the previous build computed. A CEL check
migrated from a rule file remembers the file name in the annotation
`lighthouse/migrated-from`, which the old formula hashed; a CEL decision authored
since has no legacy version.

### Verdicts

A verdict is a review event: the finding's fingerprint, rule id, `rule_version` (the
*meaning* version of the decision: a hash of its requirement, severity, scope and options,
not of its check, prose, examples or tuning notes), `check_revision` (a hash of the
`check`, recorded for evaluation and never compared to expire a verdict), `decision_hash` (the whole
decision definition), `catalog_version`, the Lighthouse version, a nullable
`pattern_fingerprint` (code-shape identity, filled by the similarity index), the
verdict, the reason, free text, the reviewer (`agent | human` and id), language, scope,
a digest of the evidence, a **snapshot** frozen at review time, the commit and a
timestamp. The snapshot is one JSON object, versioned (`"v": 2`, camelCase keys; `1`
had snake_case keys): message, path, locator, symbol, evidence, facts, options, severity,
`authored` (the severity the decision authored; builds before it recorded a `tier`; the severity string is what was recorded, so history may say `review`), when the
finding was seen (`seenAt`), the commit and dirtiness at that sighting, the Lighthouse
version and `patternFingerprint: null`. The snapshot is the single owner of the frozen evidence.

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

- the finding is not mechanical: findings of mechanical decisions are decided by the rule,
  whatever severity they are configured at, so a verdict on one is recorded but never
  suppresses it, and `check` says so (`rejected as
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
line per verdict, a `Verdict` record of the resource model (`apiVersion`, `kind`,
`metadata.name` = the record's id, `spec` with camelCase keys; canonical, compact, keys
sorted, so diffs are one line) with a single write and `fsync`, then updates the cache.
The `id` is a hash of the spec. The reader accepts both this and the flat snake_case
line that earlier builds wrote (the log is append-only, history is never rewritten),
and skips records of a kind or `apiVersion` it does not know, so new kinds never
break an older reader. Opening the store imports the entries the cache does not have and
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

`check --format agent` groups findings by decision, then by file: a header with the
requirement and the expected structure once, then one line per finding (location,
message, fingerprint prefix of at least 7 characters); `--format agent-json` is the same
groups as one JSON object. See "Compact output" in [agents.md](agents.md). `--detail full`
restores the per-finding shape described below: one block per finding that an agent can
act on without another lookup, as JSON lines (`finding`, `incomplete`, `truncated` and
`summary` records tagged by `type`) for `agent-json`:

```text
design/private-helper-callers  info (heuristic)  src/lib.rs:5:1
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
example of the decision for the file's language, at most 12 lines and 600 characters,
chosen in this order, and the block says why: the example marked `canonical` (at most
one per language and kind, enforced by the catalog validation); a valid example whose name
or whose invalid counterpart's name mentions the kind or visibility of the finding's
symbol; the shortest valid example; the decision's tuning note for the language. Evidence
that repeats the owner symbol is left out and long values are cut. Fingerprints are
shown as 12-character prefixes, longer when two shown findings would collide, and the
store accepts any unambiguous prefix. Only findings that ask for a verdict carry a
resolve command; the choices to make are the verdict and the reason, listed once after the
blocks (`reasons:`) and in the summary record of the JSON. A finding reported although a
verdict exists says why in a `note`. `--limit N` prints the N most severe findings (errors
first, original order kept) and a `... N more` tail or a `truncated` record; the summary
always counts everything. Both formats end with a summary, printed for clean runs too,
with the incomplete, suppressed and allowed counts, so silence is never read as success.

## Fixing

A rule judges, a fixer proposes, the orchestrator executes. Rules never edit, fixers
never apply, the orchestrator never judges. `check` stays pure: a fix is computed on
demand, when `check --fix` or the MCP `fix` tool asks.

**One canonical fix per rule.** The catalog is the single source that says how a rule is
fixed: the optional `fix:` block of its decision (see [Fixer spec](#fixer-spec)). The
resolution is deterministic, `finding.rule` to `decision.fix` to the fixer registered
under the decision's id; there is no list of competing fixers and no fallback chain.
`--fixer <id>` is an explicit override that must name a registered fixer; it replaces the
decision's fixer for the selected findings, whose proposals count as `suggested`, and it
is the way to use a fixer for a rule that has no `fix:`. Documentation, `explain` and the
skill show `fixable: safe|suggested`.

**Edit operations, not a round-trip IR.** Fixers do not rewrite source and there is no
lift-transform-lower pipeline (that would need a code generator per language and lose
comments and formatting). A fixer answers with `EditOp`s over code-model nodes, and the
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
span of the identifier at a reference: both are optional model fields reported by
providers that declare the capabilities `extent` and `reference-sites`. A fixer whose
required capability the file's provider lacks is declined before it is asked: never a
guess. Moves work on whole lines. A moved block keeps its blank-line separation, and the
text edit that removes it takes one adjacent blank line. A scan of brackets (comments,
strings and characters skipped) tells which container a declaration sits in: a move
needs both ends in the same one (the same file level, the same `impl` or class body), a
`reorder` sorts each container on its own, and a member of a parenthesized group (Go's
`const ( ... )`, whose order can carry meaning) is never moved. A declaration that shares
its lines with other code is not moved alone (a block comment that opens after the closing
brace counts as code). The scan reads comments and strings the way the file's language does
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
(CLI only, not offered by MCP) names a registered fixer for the selected findings and
needs `--rules` or fingerprints.

**The orchestrator** (`lighthouse-engine`, the only component that writes) runs rounds,
at most five:

1. check the whole project with every enabled rule; the run refuses to start when the
   analysis is incomplete, because a fix that rests on a partial analysis is a guess;
2. select the findings (paths, rules, fingerprints; never those a verdict suppresses),
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
   some rule and owner (the symbol, else the file), a new gap in the analysis, or a
   failed formatter; so is every file of a fix that spans files. Dropped fixes are
   reported as declined.

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
verification is the re-check on the code model. Each applied fix is recorded in the local
store (`fix_events`: finding, rule, fixer, safety, description, files, commit), not in the
shared decision log, because it records what happened to this checkout, not a decision;
the run that ends the fixing resolves the findings the fixes removed.

Bundled fixes: `design/declaration-groups` (a `reorder` by `design/group`, plus a `move`
for constructors) and `testing/test-file-layout` (a `move` of a fixture above, or a helper
below, the tests) are safe; `design/related-symbols-close` and
`design/callers-before-callees` (a `move` next to the related symbol or the caller),
`design/section-banners` and `core/unused-allow` (a `delete` of the comment or the
annotation line) are suggested. An `impl` block is not a symbol, so
reordering a Rust file cannot move one: such findings are declined and fixed by hand.
`lighthouse check .` on this repository is clean, and `check --fix` brought it there.

## Fixer spec

The `fix:` block of a decision is declarative and selects one fixer *kind*; every kind
compiles into a provider of the single `Fixer` interface, the way declarative rules
compile into `Rule`, and bundled fixers are such specs registered by their plugin through
`Plugin::fixers()` under the decision's id. Local rules in `.lighthouse/decisions/*.yaml` carry
a `fix:` too.

```yaml
fix:
  safety: safe            # safe | suggested: the cap on what the fixer may claim
  requires: [extent]      # optional provider capabilities; absent means declined
  # exactly one of:
  ops:                    # CEL over the finding and the code model
    - op: move
      node: finding.evidence.callee
      after: finding.evidence.caller
  command:
    argv: ["gofmt", "-s", "-w", "{file}"]   # no shell
    output: inPlace       # inPlace | text (the new file text on stdout)
    stdin: none           # none | file
    env: {}               # extra variables; the rest is cleared
    timeout: 30s
    scope: file           # edits outside the finding's file are refused
  rpc: {}                 # reserved: a language plugin's own fix method; not yet supported
```

- **`ops`** is a list of generic operations: `move`, `reorder`, `delete`, `rename`,
  `replace`. Each takes typed parameters, validated by the catalog; expressions are CEL over
  `finding` (`rule`, `message`, `file`, `span`, `symbol`, `evidence`, `fingerprint`,
  `facts`), `symbol` (the finding's symbol, as in declarative rules) and `options`, and an
  optional `when` skips an operation. `text` and `name` are templates with `{{ cel }}`
  holes. The operations, their parameters and the registered order keys of `reorder` are
  generated into [decisions/fix-operations.md](decisions/fix-operations.md). Semantics that
  need Rust (`reorder` sorting by a key such as `design/group`, registered by a plugin
  through `Plugin::order_keys()`) live in the operation, not in per-rule code.
- **`command`** is the simple text contract; structured data goes through `rpc`. It runs a
  program without a shell in a scratch directory that holds a copy of the finding's file.
  *Exit codes:* `0` succeeds with the result per `output` (an empty result changes nothing),
  `1` declines with the reason taken from stderr, `>= 2`, a signal or a timeout is an error:
  nothing is applied and the capped stderr goes into the notice. *Output:* `inPlace` diffs
  the scratch copy against the original into the smallest whole-line `Replace`; `text` takes
  the new file text from stdout, which carries only the result (stderr is never parsed).
  *stdin:* `none` (default) or `file`, the target file's content; no request JSON is sent.
  *Argv:* `{file}`, `{line}`, `{symbol}` and `{rule}` fill a whole argument each (no
  embedding, no `{root}`; a value starting with `-` is refused); the working directory is the
  scratch directory, and a symlink in the output is refused. *Environment:* cleared to
  `PATH`, `HOME`, `LANG`, `TMPDIR`, plus the declared `env`, plus `LIGHTHOUSE_DECISION` (the
  finding as JSON), `LIGHTHOUSE_OPTIONS` (the rule's options as JSON) and
  `LIGHTHOUSE_API_VERSION`. *Limits:* a timeout sends SIGTERM to the process group, then
  SIGKILL after a grace period, through the runner shared with the plugin host
  (`lighthouse-process`); stdout and stderr are capped. The result goes through the same
  containment, verify and rollback as any other fix.
  **Trust:** commands (command fixers and `[languages.<id>] formatter`) run only in a
  project the *user* trusts, whoever wrote them. `lighthouse trust` lists the formatters and
  fixer commands it would trust and asks for confirmation on the terminal (without a
  terminal it refuses unless `--yes` says the list was read; `init --agent claude-code`
  adds `Bash(lighthouse trust:*)` to the agent's permission deny list); it records the
  canonical root with a digest of `lighthouse.toml`, the rule files and the content of any
  command program that lies inside the project (a program named by a relative path is the
  project's own, run from the project root) in `~/.lighthouse/trust.toml`. Programs outside
  the project (`gofmt`, `rustfmt`) are trusted by command line only. Every field of the digest
  is tagged and length-prefixed, and it is computed from the very bytes the configuration
  and rules were loaded from. Authoring candidates (`decision_create`) are never trusted (`$LIGHTHOUSE_HOME` replaces `~/.lighthouse`); any change to
  either withdraws it, and `lighthouse trust --revoke` removes it. `LIGHTHOUSE_TRUST=1`
  trusts every project, for CI. The repository cannot grant trust itself: a `[fix]` table
  in `lighthouse.toml` is not accepted. Without trust a command fix is declined with a
  message naming the command, and formatting is skipped with a note.
- **Validation** (every catalog load, local layers included): exactly one kind; a decision
  with an implementation; `safe` only on mechanical decisions; every CEL expression and
  template compiles; operation parameters are well-typed (one of `before`/`after`, `node`
  or `file` and `span`, qualified order keys); a command has a program and a timeout;
  `rename` lists `complete-references`; placeholders fill whole arguments; an `rpc` fix is
  kept but only that rule's findings are declined as not yet supported (it does not fail
  the pack); and the decision has an invalid example with
  `fixed`. The workspace tests also check that every bundled `fix` has its rule and a
  registered fixer, that every `reorder` key is registered and that the `requires` list
  covers what the operations need.
- **Testing:** an invalid example may carry `fixed:`, the new text of each file that
  changes. `lighthouse rule test` applies the fix to the example (suggested ones too),
  asserts the files equal `fixed` (a final newline aside), checks that the rule no longer
  fires and that applying the fix again changes nothing. `decision_create` and `decision_update`
  run the same examples in their gate.

## Crates

Dependencies point down; the only exceptions are marked and go away with the
move of the project model into `spec` (`spec` → `plugin`, `plugin` → `config`).

```
L5  cli, mcp                  -> session, report            (nothing else: no store, registry or engine)
L4  session                   -> every layer below          (composition root; owns every legacy reader: `migrate`)
    report                    -> model, spec
L3  store                     -> model, resource            (findings, verdicts, the decision log)
L2  engine                    -> plugin, config, spec, model, process   (analysis, fix orchestration, rule tester)
    checks                    -> plugin, spec, model, process, resource (declarative checks, metrics, order keys,
                                                                       the bundled registry derived from the catalog's packs)
    rpc                       -> plugin, config, protocol, process, model, resource  (language plugins as processes)
L1  plugin                    -> model, config, resource    (SPI and Registry)
    spec                      -> model, plugin, resource    (decision kinds, Catalog, local layer)
    config                    -> model, resource            (Project, rule levels, overrides)
L0  model, resource, protocol, process   (no workspace dependencies)
```

`lighthouse-checks` holds everything the bundled plugins run: the declarative
compilation of a decision's `check` and `fix`, the `metrics/*` analyzers, the
order keys of the `design` pack and the text fallback and annotation rules of
`core`. `checks::registry()` builds one `Declarative` plugin per pack of the
bundled catalog and adds those; plugin ids and rule ids are the packs' own.
`lighthouse-test-support` is the dev-only helper of the test suites (plugin
builds, project documents, catalog files). Each crate has one integration-test
binary, `tests/it/main.rs` with a module per area, and no lib test binary unless
it has unit tests.

## Frontends

The CLI, the MCP server and the agent hooks are frontends over one shared layer,
`lighthouse-session`: it loads a project (configuration, local rules, catalog),
runs a check and remembers it, records verdicts, tests and authors rules, and
renders the agent skill. A frontend parses its own input and prints the result;
it holds no logic of its own, so a verdict recorded through MCP, the CLI or a
hook is the same event in the same log. Fixing is the same: `check --fix [--dry-run]
[--unsafe-fixes] [--fixer <id>]` and the MCP `fix` tool call one session operation.
Hooks never fix.

```
 lighthouse-cli ── check [--fix], review, decision, spec, schema, docs, init --agent, hook claude-code
 lighthouse-mcp ── `lighthouse mcp`: tools and resources over stdio (rmcp)
        └── lighthouse-session ── engine, store, spec, checks, rpc, report
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
bundled `core`, `design` and `testing` plugins with their recommended presets (`<plugin>/strict` adds the advice that is too noisy to recommend, such as `design/private-helper-callers`) and
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
