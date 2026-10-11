# Roadmap

Lighthouse is an architecture decision compiler: it turns engineering decisions into
executable rules and keeps learning which decisions can be enforced deterministically.
Lint executes rules; Lighthouse makes rules from decisions.

The core primitive is **decision → judgment → evidence → rule evolution**. Turning an ADR
into a check is not enough on its own: ADR-as-code tools already do it. What Lighthouse
adds is:

1. **Decision memory.** Every finding, judgment, reason and evidence snapshot is linked to
   the decision that produced it, shared through git, and expires when meaning or
   evidence changes. This history is the asset; models are replaceable.
2. **A semantic code model.** Native language providers build one language-neutral model
   (symbols, extents, edges, metrics). Rules are written once against that model instead
   of grep scripts per rule.
3. **Checks that evolve without losing history.** A decision can start as plain text and
   is enforced from day one by a model or an agent answering a prompt. Later check
   revisions bind a model trained on its judgments and, when the boundary can be expressed
   faithfully, a deterministic rule. Each revision passes an evaluation against recorded
   judgments and needs explicit approval. Judgments survive, because they are tied to the
   decision's meaning, not to its check.
4. **Measured trust.** Each decision's precision is computed from its judgments. Noisy
   decisions get narrowing or demotion proposals.

The stages from source to promoted rule are described in [rule-pipeline.md](rule-pipeline.md).

Depth comes before breadth: the loop must work end to end on Go and Rust before more
languages and editors are added.

## Done

| Step | Delivered |
| --- | --- |
| Foundation | Rust workspace, plugin contract, config, engine, reporters |
| Decision catalog | one spec per decision, generated docs, executable valid/invalid examples |
| Analysis | metrics analyzers (complexity, nesting, coupling), first design rules |
| Language providers | JSON-RPC protocol; Go provider in Go (`go/packages`, `go/types`); Rust provider; conformance suite |
| Self-enforcement | layout, test and declarative (CEL) rules; Lighthouse lints itself in `make lint` |
| Decision memory | SQLite history, committed `decisions.jsonl`, `lighthouse-disable-next-line` directives, expiring judgments, evidence snapshots, `--format agent` |
| Agent loop | MCP server, Claude Code hooks, generated skill, `init --agent`; compact grouped agent output with fix previews |
| Autofix | one canonical fix per decision (`ops`, `command`), in-memory verification, rollback, atomic writes, user-level trust |
| Resource model | `apiVersion`/`kind`/`metadata`/`spec` for every spec, JSON Schema per kind, `spec validate`, SARIF, `pattern` renamed to `decision`, severity `error`/`warn`/`info` |
| Check providers | `check:` as `builtin` (standard ops `order`, `proximity`, `cycle`), `cel` with a standard library, `command`, `rpc` (reserved) or `model` (served by agent review tasks); every bundled decision re-expressed with identical findings; `severity` replaces `enforcement`; meaning version separate from check revision; ADR `status`, `supersedes`, `consequences` |
| Speed | per-run fact memo, parallel rule and provider runs (rayon), concurrent Go modules, `--timings` |
| Fewer concepts | `spec::project` replaces config (projects replace presets and overrides); fields cut to requirement/scope/severity/options + context/consequences/status/supersedes + check/fix/examples/provenance; generated and test code handled by each decision's scope; camelCase options; UUID identity; decision ids in the ESLint convention, checked by `core/decision-naming`; ESLint-style `lighthouse-disable` directives; judgments (`pass`/`fail`/`notApplicable`) and SARIF suppressions replace verdicts; `decisions.jsonl` is the only source of truth |
| Self-check and placement | `layers` (import-linter contracts), `unique-type-names`, `tiny-modules`, `feature-envy` and `misplaced-symbol` (Lanza & Marinescu thresholds), `owner-file` (file in Go, module tree in Rust), `no-hidden-target`; enum variants as a symbol kind; option shapes validated |
| Rules aligned with established tools | independent standard limits (cyclomatic, cognitive, statements, depth, params, results, function length) with one limit shape per role and `-1` for none; params and results count the fields of single-use structs; model checks made deterministic where a linter proves it (context-first, stored context, error identity, panics, private types in APIs); `command` checks read SARIF 2.1.0 (`output: sarif`, `select`, `columns`) so common linters are [wrapped](wrapping-linters.md); the [baseline](baseline.md): precision per decision, timings and findings on the reference repositories |

## Next

Rule accuracy and checking cost are measured; the [baseline](baseline.md) is the yardstick.
Rules first, then models where rules fall short, then reach.

### Signals that earn their place
- **Measured info signals:** a deterministic check below 20% precision after cheap fixes
  stops deciding. Its heuristic becomes the recall-oriented candidate selector of a model
  check, merged into the decision that already states the requirement. The rest are
  refined at their measured false-positive causes.
- **One cohesive catalog:** similar decisions merge. Variants become options or kinds,
  every finding carries its kind so precision stays measurable, and other linters'
  granularity is consulted. 117 decisions become 29.
- **Clean Code, fully covered:** every item of Martin's *Smells and Heuristics* and every
  chapter rule is covered by a decision, recorded as not applicable, or pending a named
  fact. A coverage test enforces it.
  - A new deterministic kind ships only if its sampled precision passes; otherwise it is
    judged.
  - A project can replace a bundled decision's check (`rules."<id>".check`) to wrap a
    linter it already runs.
  - New facts are general shapes: flow attributes, event kinds, external calls, parameter
    attributes, type origin. Effects, locals and clone fingerprints follow.

### Models where rules fall short
A decision says what to ask; the project binds who answers. Bindings resolve in layers,
and an abstention falls through: a classifier trained for this decision, then a model
bound per decision, then the project's model, then the agent.
- **Runtime:**
  - a check is decided by its rule, which abstains where it is unsure;
  - candidates go through the layers;
  - undecided cases are reported as `review`;
  - answers are cached, `cost > 0` calls are budgeted and limited to the report scope,
    and remote use is opt-in.
- **Local first, at no cost:** an embedding model run in process (EmbeddingGemma) and a
  per-decision classifier (LightGBM against kNN and logistic-regression baselines,
  calibrated, abstaining). It is trained on strength-weighted judgments and never on its
  own outputs.
- **Remote decision models:** System One models through OpenRouter's Decisions API
  answer with a probability. `:free` chat models label candidates on request, and the
  local classifier distils those labels.
- **Evaluation is the promotion gate (CLI and MCP):** precision, false-positive rate,
  recall, false-negative rate, selector precision, coverage and agreement, with
  intervals, over examples, hand-reviewed samples and judgments. A trained model is
  attached to a decision only by an approved revision that beats the current check.
  Sampled re-judging gives recall and drift.
- **Generative fixes:** a fix may be synthesised by the bound generation model (an API
  model, or any CLI as a `command` model). It is always verified (apply, format, re-check, roll back) and
  always `suggested`. Unbound, the agent gets a fix task.

### Fast on large repositories
- **Done:** per-unit provider caches with early cutoff and a host result cache keyed by
  each rule's declared reach. Warm runs are byte-identical to cold runs
  (`scripts/fastgate.sh`).
- **Next:**
  - type-check edited packages from source over dependency export data;
  - cache failed units;
  - let the host keep fragments, so providers answer "unchanged" (the index half of
    protocol 0.2).
- **Target:** a one-file edit costs under 40% of a cold run, reported per repository.
- **Protocol 0.2, the rest:** LSP lifecycle, text sync, UTF-8 positions, and
  `lighthouse/check` for `rpc` checks.

### Packaging
- Prebuilt binaries that bundle the Go and Rust providers and the local model plugin
  (model weights download on first use), plus a one-command install. Verified on clean
  machines.
- Not yet a public release.

### The decision loop, completed
- **Graph queries:** decision, finding, judgment (reason, actor), evidence and revision.
- **Proposals:** narrow, widen, demote or promote a decision, with generated examples
  and drafted wording. Nothing is enabled automatically.
- `lighthouse log compact`.

### Decision search
- One `search` tool over decisions, judgments, findings, examples, symbols and
  revisions, for decisions that rules cannot capture precisely.
- **Ranking:** a structured filter, then BM25 (FTS5), the project's embedding model and
  structural signals, fused with Reciprocal Rank Fusion.
- **Hooks** search when a task starts and before a file's first edit, first in shadow
  mode. Agents' answers become judgments.
- **Evaluation:**
  - History labels are weak: a missing record is unlabelled, never irrelevant.
  - The final measure is a TREC-pooled, hand-verified subset.
  - Splits are by repository and commit time; tuning data is kept apart.
  - Results are reported per check type, decision and language.
  - FTS5 BM25 alone is the baseline; embeddings are adopted only if they improve it.

### Adoption
Public adoption waits until the loop has been shown working.
- Public release.
- `lighthouse init` detects languages and proposes a starter set, including mined
  decisions.
- A docs pass: each document holds only what its reader needs.

### Learning from history
- **Repository mining:** refactor-like commits and dominant conventions become weak
  labels and `proposed` decisions. Changes are grouped by structural change first, then
  by embeddings; generation drafts the wording.

### Later
| Step | Scope |
| --- | --- |
| Artifact graph | domain-neutral nodes and edges; a markdown provider as the first non-code domain |
| Breadth | `lsp-bridge` (any off-the-shelf language server, at lower capability), then native TypeScript and Python providers |
| Ecosystem | `lighthouse lsp` for editors (diagnostics, fixes, judgments); external rule plugins over RPC that ship their decision specs; `plugin add` with a lockfile |
| Session domain | decisions about agent actions; a PreToolUse gate that allows, asks or denies |
| More model transports | Anthropic Messages API; session-domain models |

## Known gaps
- A per-language check, `spec.languages.<id>.check`, does not exist. A decision whose
  deterministic part rests on facts only some providers give (`design/context-first`,
  `no-stored-context`, `error-identity`, `no-private-types`: Go only) guards its `where` on
  the language and cannot keep a `model` check for the others.
- `testing/no-hidden-target` cannot tell a helper that returns or asserts on the target's
  result from one that only builds with it (a data-flow fact: built or consumed against
  returned or asserted), so it is info.
- `design/layers` does not report an `ignore` entry that matches no import (import-linter's
  `unmatched_ignore_imports_alerting`): a cel rule judges one edge at a time.
- Rename fixes are off: no provider declares complete reference sites yet.
- External binaries in `command` providers are trusted by their command line, not by
  content.
