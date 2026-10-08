# Roadmap

Lighthouse is an architecture decision compiler: it turns engineering decisions into
executable rules and keeps learning which decisions can be enforced deterministically.
Lint executes rules; Lighthouse makes rules from decisions.

The core primitive is **decision → judgment → evidence → rule evolution**. Turning an ADR
into a check is not enough on its own: ADR-as-code tools already do it. What Lighthouse
adds is:

1. **Decision memory.** Every finding, verdict, reason and evidence snapshot is linked to
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
4. **Measured trust.** Each decision's precision is computed from its verdicts. Noisy
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
| Decision memory | SQLite history, committed `decisions.jsonl`, `lighthouse:allow` annotations, expiring verdicts, evidence snapshots, `--format agent` |
| Agent loop | MCP server, Claude Code hooks, generated skill, `init --agent` |
| Autofix | one canonical fix per decision (`ops`, `command`), in-memory verification, rollback, atomic writes, user-level trust |
| Resource model | `apiVersion`/`kind`/`metadata`/`spec` for every spec, JSON Schema per kind, `spec validate`/`migrate`, SARIF, `pattern` renamed to `decision`, severity `error`/`warn`/`info` |

## Next

### 2d-2: one provider model for checks
- `check:` takes one provider, like `fix:` does: `builtin`, `cel`, `command`, `rpc` or
  `model`. Whether a check is deterministic is a capability the provider declares, not a
  type. `type: model` asks the bound model the decision's requirement, with its examples
  as shots; agent review tasks serve it for now.
- Builtins become standard, decision-agnostic operations:
  - Per-element predicates are written in `cel`, with a standard function library
    (`metrics`, `callers`, `edges`, `tests`, `annotations`, …).
  - Aggregate checks use builtin `order`, `proximity` and `cycle`.
  - Analyzers compute facts; checks only judge.
  - Every bundled decision is re-expressed this way, and the gate is identical findings
    before and after.
- `enforcement` is removed. A decision declares `severity: error|warn|info`:
  - `error` is definitive: it needs no verdict, and only an annotation can waive it.
  - `warn` and `info` are review tasks.
  - A decision without `check:` is documentation only.
- ADR lifecycle fields: `status`, `supersedes`, `rationale`, `consequences`, `provenance`.
- The decision version is split in two:
  - The *meaning version* (requirement, scope, severity, options) decides whether a
    verdict still holds.
  - The *check revision* only records which implementation produced a finding.
  - Promoting a decision to a better check therefore keeps its verdicts.
- `command` checks follow the process contract:
  - exit code `0` clean, `1` findings, `≥2` error;
  - findings are stdout lines, with an optional `path:line:col:` prefix.
- Agent output is compact:
  - MCP `check`, `review_tasks`, `--format agent` and hooks group findings by decision,
    then by file;
  - each decision's requirement, expected example and verdict instructions appear once;
  - one finding is one short line: location, message and fingerprint prefix;
  - the full per-finding shape is available with `detail: full`.
- Plugin protocol 0.2 is LSP-shaped:
  - LSP lifecycle, with Lighthouse capabilities under `experimental`;
  - text sync replaces overlays;
  - UTF-8 positions;
  - `lighthouse/index`, `lighthouse/check` and `lighthouse/fix` methods.

### 2d-3: fewer concepts
- Decision fields shrink to:
  - meaning: `requirement`, `scope`, `severity`, `options`;
  - record: `title`, `context`, `consequences`, `status`, `supersedes`;
  - `check`, `fix`, `examples` and `provenance`.
- Merged away:
  - `intent` and `rationale` become `context`;
  - `exceptions` moves into the requirement;
  - `citation` moves into provenance;
  - declared `evidence` comes from the check;
  - `strict` becomes a label;
  - per-language `tuning` is replaced by examples.
- Kinds: `Preset` becomes a shareable `Project`, `DecisionOverride` folds into project rules, and `SourceMap` moves into provenance.
- Verdicts become judgments (`pass`, `fail`, `notApplicable`) and suppressions. Signals and fix records become evidence.
- Names follow standards (MADR, SARIF, W3C PROV-O). `spec migrate` converts existing files, and judgments survive.

### 3: decision graph and evaluation
- Queries over the decision graph: decision, finding, verdict with reason and actor,
  evidence, and revision.
- Judgments on subjects, not only on findings:
  - A `model` check takes candidate subjects from the decision's scope, and an agent or model records
    `pass` or `fail` for each.
  - A fraction of already decided subjects is sampled and re-judged.
  - Together these give labels for recall, not just precision.
- Per-decision precision and estimated recall.
- An evaluation harness that replays a candidate check against recorded verdicts and all
  examples, and measures agreement with the previous tier. This is the promotion gate
  used by people and agents alike, via CLI and MCP.
- Structural similarity (normalized AST and token shingles, MinHash) and an optional
  `Model` plugin kind (task `embedding`) as retrieval aids, never as deciders.
- Baseline and ratchet; cycle, clone and stability analyzers.
- `lighthouse log compact` folds expired and superseded events in `decisions.jsonl`;
  the full history stays in git.

### 4: decision evolution
- Signals are captured from verdicts, annotations, fix outcomes and decision edits.
- Signals are grouped into `proposed` decisions.
- The repository itself is a signal source. Refactor-like commits label the old code
  as violating and the new code as conforming, and in a cluster of similar code the
  dominant shape counts as conforming. Changes are grouped by their structural change
  first and refined with a local embedding model.
- Proposals narrow, widen or demote a decision, with generated examples.
- Check revisions (attach a learned model, add a deterministic part, demote) go through
  the evaluation gate. They are routing by cost and confidence, not a maturity state, and
  nothing is enabled automatically.

### 5: adoption
- Prebuilt release binaries that bundle the Go and Rust providers, and a one-command
  install.
- `lighthouse init` detects languages and proposes a starter set of decisions, including
  decisions mined from the repository's own history and conventions.

### Later
| Step | Scope |
| --- | --- |
| Artifact graph | domain-neutral nodes and edges; a markdown provider as the first non-code domain |
| Breadth | `lsp-bridge` (any off-the-shelf language server, at lower capability), then native TypeScript and Python providers |
| Ecosystem | `lighthouse lsp` for editors (diagnostics, fixes, verdicts); external rule plugins over RPC that ship their decision specs; `plugin add` with a lockfile |
| Learning | per-decision routing: a classifier trained on that decision's judgments decides only when confident and passes the rest to a prompted model or an agent. Models are opt-in, cached and budgeted. A model must beat a statistical baseline before use, and a sample of its confident calls is still re-judged. |
| Session domain | decisions about agent actions; a PreToolUse gate that allows, asks or denies |

## Known gaps
- Rename fixes are off: no provider declares complete reference sites yet.
- External binaries in `command` providers are trusted by their command line, not by
  content.
- `spec migrate` drops comments in TOML.
