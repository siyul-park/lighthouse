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
3. **An evolution ladder.** A decision can start as plain text and is enforced from day
   one by judgment. It is promoted to a learned detector, then to a deterministic rule.
   Each step passes an evaluation against recorded verdicts and needs explicit approval.
4. **Measured trust.** Each decision's precision is computed from its verdicts. Noisy
   decisions get narrowing or demotion proposals.

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
- `check:` takes one provider, like `fix:` does: `builtin`, `cel`, `command`, `rpc`, and a
  reserved `judged` slot (served for now by agent review tasks).
- Builtins become standard, decision-agnostic operations:
  - Per-element predicates are written in `cel`, with a standard function library
    (`metrics`, `callers`, `edges`, `tests`, `annotations`, …).
  - Aggregate checks use builtin `order`, `proximity` and `cycle`.
  - Analyzers compute facts; checks only judge.
  - Every bundled decision is re-expressed this way, and the gate is identical findings
    before and after.
- `enforcement` is removed. A decision declares `severity: error|warn|info`:
  - `error` is a deterministic contract that only an annotation can waive.
  - `warn` and `info` are review tasks.
  - A decision without `check:` is documentation only.
- ADR lifecycle fields: `status`, `supersedes`, `rationale`, `consequences`, `provenance`.
- `command` checks follow the process contract:
  - exit code `0` clean, `1` findings, `≥2` error;
  - findings are stdout lines, with an optional `path:line:col:` prefix.
- Plugin protocol 0.2 is LSP-shaped:
  - LSP lifecycle, with Lighthouse capabilities under `experimental`;
  - text sync replaces overlays;
  - UTF-8 positions;
  - `lighthouse/index`, `lighthouse/check` and `lighthouse/fix` methods.

### 3: decision graph and evaluation
- Queries over the decision graph: decision, finding, verdict with reason and actor,
  evidence, and revision.
- Per-decision precision.
- An evaluation harness that replays a candidate check against recorded verdicts and all
  examples. This is the promotion gate used by people and agents alike, via CLI and MCP.
- Structural similarity (normalized AST and token shingles, MinHash) and an optional
  `Embedder` plugin kind as retrieval aids, never as deciders.
- Baseline and ratchet; cycle, clone and stability analyzers.

### 4: decision evolution
- Signals are captured from verdicts, annotations, fix outcomes and decision edits.
- Signals are grouped into `proposed` decisions.
- Proposals narrow, widen or demote a decision, with generated examples.
- Promotion `judged → learned → deterministic` through the evaluation gate. Nothing is
  enabled automatically.

### Later
| Step | Scope |
| --- | --- |
| Artifact graph | domain-neutral nodes and edges; a markdown provider as the first non-code domain |
| Breadth | `lsp-bridge` (any off-the-shelf language server, at lower capability), then native TypeScript and Python providers |
| Ecosystem | `lighthouse lsp` for editors (diagnostics, fixes, verdicts); external rule plugins over RPC that ship their decision specs; `plugin add` with a lockfile |
| Learning | statistical baseline, learned detectors gated on beating it, Judge providers (opt-in, cached, budgeted) |
| Session domain | decisions about agent actions; a PreToolUse gate that allows, asks or denies |

## Known gaps
- Rename fixes are off: no provider declares complete reference sites yet.
- External binaries in `command` providers are trusted by their command line, not by
  content.
- `spec migrate` drops comments in TOML.
