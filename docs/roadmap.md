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

## Next

Rule accuracy and checking cost are proven before anything is added on top. The
precision and timing measured here are the baseline that search and learned checks are
later judged against.

### R2: self-check and placement rules (in review)
- **Self-check:** `layers` (import-linter contracts), `unique-type-names`, `tiny-modules`.
- **Placement:** `feature-envy`, `misplaced-symbol`, a deterministic `owner-file`, and
  `no-hidden-target`.
- **Gate:** findings of every other rule are identical on four repositories.
- **Precision review:** each new rule is reviewed with at least 20 sampled findings
  (or all of them, if fewer), with a Wilson interval reported. False-positive causes are
  fixed in the code model or the rule's definition, not with ad-hoc exclusions.

### R3: rules aligned with established tools
- **Limits:**
  - `complexity` splits into independent standard limits: cyclomatic, cognitive,
    statements, depth, parameters, function length;
  - `max-params` and `max-results` count the fields of single-use parameter structs;
  - one limit shape with values per role (constructor, implementation, entrypoint, test).
- **Model to deterministic:** model checks become deterministic where a tool shows it can
  be done (context-first, stored context, error identity, panics, private types in APIs).
- **Commodity checks are wrapped, not rewritten:** a decision can be enforced by an
  existing linter (golangci-lint, clippy, ruff) through a `command` check, and Lighthouse
  adds the decision, memory and judgments on top.
- **Wrapped output is normalized:**
  - SARIF 2.1.0 is read as a standard input format;
  - each tool rule maps to a decision;
  - locations use UTF-16 columns per SARIF;
  - fingerprints come from the tool rule, the location and the normalized message
    (or the tool's `partialFingerprints`), so they stay stable across tool versions.
- **Baseline report:** per-decision precision from sampled findings and from judgments,
  plus check time per phase on the reference repositories.

### Fast on large repositories
- **Caches:** content-addressed caches with early cutoff, the approach of Go build action
  IDs, Salsa and ESLint `--cache`:
  - per-unit index (with plugin protocol 0.2);
  - per-file results.
- **Protocol 0.2:** LSP lifecycle, text sync, UTF-8 positions, and `lighthouse/check`
  for `rpc` checks.
- **Gate:** a warm run equals a cold run byte for byte, and a one-file edit re-checks only
  what depends on it.

### Packaging
- Prebuilt binaries that bundle the Go and Rust providers, and a one-command install,
  verified on clean machines early, so real-environment problems surface before the
  loop work.
- Not yet a public release.

### The decision loop
This is the core value: a decision starts as text, is enforced at once by agent review,
and earns a deterministic check through measured evidence.
- **Graph queries:** decision, finding, judgment (reason, actor), evidence and revision.
- **Labels for recall:** judgments on candidate subjects of `model` checks, plus sampled
  re-judging of decided subjects.
- **Evaluation harness (the promotion gate, via CLI and MCP):**
  - replays a candidate check against recorded judgments and every example;
  - measures precision, estimated recall and agreement with the previous check.
- **Proposals:** narrow, widen, demote or promote a decision, with generated examples.
  Nothing is enabled automatically.
- `lighthouse log compact`.

### Decision search
- One hybrid `search` tool over decisions, judgments, findings, examples, symbols and
  revisions, for decisions that rules cannot capture precisely.
- **Ranking:** a structured filter first, then BM25 (FTS5), a local embedding model and
  structural signals, fused with Reciprocal Rank Fusion.
- **Hooks:** they search decisions when a task starts and before a file's first edit,
  first in shadow mode.
- **Feedback:** agents' answers (applied, not applicable, knowingly not followed) become
  judgments and refinement proposals.
- **Evaluation:** labels come from history. Recall@k, nDCG@10 and MRR are measured
  against the R3 baseline.
- **Evaluation labels.**
  - **History labels are weak.** A finding or judgment marks a decision as relevant to
    a file. A missing record is unlabelled, never "irrelevant": history keeps only the
    cases someone judged.
  - **A verified subset is the final measure.** It uses TREC-style pooling: the top
    results of every ranker are pooled and the pool is judged by hand.
  - **Splits:** by repository and by commit time. Data used to tune the rankers (fusion
    constant, boosts, k) is never used for the final evaluation.
  - **Strata:** results are reported per check type, decision and language, because
    history over-represents decisions with deterministic checks and files that already
    have findings. Judgments of `model` decisions and recorded `notApplicable` answers are
    included.
  - **Baseline:** FTS5 BM25 alone is measured first. The embedding model is adopted only
    if it improves the verified-subset metrics.

### Adoption
Public adoption waits until the decision loop has been shown working.
- Public release.
- `lighthouse init` detects languages and proposes a starter set of decisions, including
  decisions mined from the repository's history.
- A docs pass: each document holds only what its reader needs.

### Learning from history
- **Repository mining:** refactor-like commits and dominant conventions become weak
  evidence and `proposed` decisions. Changes are grouped by structural change first, then
  by embeddings.
- **Learned routing per decision:** a calibrated classifier decides only when confident,
  and must beat a statistical baseline and kNN.

### Later
| Step | Scope |
| --- | --- |
| Artifact graph | domain-neutral nodes and edges; a markdown provider as the first non-code domain |
| Breadth | `lsp-bridge` (any off-the-shelf language server, at lower capability), then native TypeScript and Python providers |
| Ecosystem | `lighthouse lsp` for editors (diagnostics, fixes, judgments); external rule plugins over RPC that ship their decision specs; `plugin add` with a lockfile |
| Session domain | decisions about agent actions; a PreToolUse gate that allows, asks or denies |

## Known gaps
- Rename fixes are off: no provider declares complete reference sites yet.
- External binaries in `command` providers are trusted by their command line, not by
  content.
