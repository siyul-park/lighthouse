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
| Agent loop | MCP server, Claude Code hooks, generated skill, `init --agent` |
| Autofix | one canonical fix per decision (`ops`, `command`), in-memory verification, rollback, atomic writes, user-level trust |
| Resource model | `apiVersion`/`kind`/`metadata`/`spec` for every spec, JSON Schema per kind, `spec validate`, SARIF, `pattern` renamed to `decision`, severity `error`/`warn`/`info` |
| Check providers | `check:` as `builtin` (standard ops `order`, `proximity`, `cycle`), `cel` with a standard library, `command`, `rpc` (reserved) or `model` (served by agent review tasks); every bundled decision re-expressed with identical findings; `severity` replaces `enforcement`; meaning version separate from check revision; ADR `status`, `supersedes`, `consequences` |

## Next

### 2d-2b: compact agent output and protocol 0.2
- Agent output is compact:
  - MCP `check`, `review_tasks`, `--format agent` and hooks group findings by decision,
    then by file;
  - each decision's requirement, expected example and judgment instructions appear once;
  - one finding is one short line: location, message and fingerprint prefix;
  - the full per-finding shape is available with `detail: full`.
- Plugin protocol 0.2 is LSP-shaped:
  - LSP lifecycle, with Lighthouse capabilities under `experimental`;
  - text sync replaces overlays;
  - UTF-8 positions;
  - `lighthouse/index`, `lighthouse/check` and `lighthouse/fix` methods, which make
    `rpc` checks real.

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
- Judgments (`pass`, `fail`, `notApplicable`) and suppressions replace verdicts (done). Signals and fix records become evidence.
- Names follow standards (MADR, SARIF, W3C PROV-O).

### 3: decision graph and evaluation
- Queries over the decision graph: decision, finding, judgment with reason and actor,
  evidence, and revision.
- Judgments on subjects, not only on findings:
  - A `model` check takes candidate subjects from the decision's scope, and an agent or model records
    `pass` or `fail` for each.
  - A fraction of already decided subjects is sampled and re-judged.
  - Together these give labels for recall, not just precision.
- Per-decision precision and estimated recall.
- An evaluation harness that replays a candidate check against recorded judgments and all
  examples, and measures agreement with the previous tier. This is the promotion gate
  used by people and agents alike, via CLI and MCP.
- Structural similarity (normalized AST and token shingles, MinHash) and an optional
  `Model` plugin kind (task `embedding`) as retrieval aids, never as deciders.
- Baseline and ratchet; cycle, clone and stability analyzers.
- `lighthouse log compact` folds expired and superseded events in `decisions.jsonl`;
  the full history stays in git.

### 4: decision evolution
- Signals are captured from judgments, directives, fix outcomes and decision edits.
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
| Ecosystem | `lighthouse lsp` for editors (diagnostics, fixes, judgments); external rule plugins over RPC that ship their decision specs; `plugin add` with a lockfile |
| Learning | per-decision routing: a classifier trained on that decision's judgments decides only when confident and passes the rest to a prompted model or an agent. Models are opt-in, cached and budgeted. A model must beat a statistical baseline before use, and a sample of its confident calls is still re-judged. |
| Session domain | decisions about agent actions; a PreToolUse gate that allows, asks or denies |

## Known gaps
- `design/layers` does not report an `ignore` entry that matches no import (import-linter's
  `unmatched_ignore_imports_alerting`): a cel rule judges one edge at a time.
- Rename fixes are off: no provider declares complete reference sites yet.
- External binaries in `command` providers are trusted by their command line, not by
  content.
