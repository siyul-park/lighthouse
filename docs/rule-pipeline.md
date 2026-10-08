# Rule pipeline

This page covers how a decision gets a rule, and how that rule is judged, learned and refined.
Every path ends in the same place: a `Decision` resource whose `check:` sits on one stage of
a ladder, plus a decision memory that moves it up or down. The [roadmap](roadmap.md) says
which parts exist today (see [Status](#status)).

Invariants that hold on every path:
- **A decision is the source of truth.** Prompts, models, thresholds and examples derive from it and can be rebuilt.
- **Nothing enables itself.** Proposals, new models and promotions all need explicit approval.
- **Each decision is its own learning unit.** No single model judges every decision.
- **Precision before coverage.** A stage that is not confident passes the case on; it never guesses.
- **Every judgment becomes history.** It is recorded and becomes a label for the next round.
- **The goal is deterministic enforcement.** Judges and models are intermediate forms, not the product.
- **The core has no LLM API.** Judges, embedders and detectors are plugins, local first and opt-in when remote.

## Creation paths

```text
 1 authored ───────────────────────────────┐
 2 authored from history ──(subcase of 1)──┤
 3 prompted judge ─────────────────────────┤
 4 mined from change history ──────────────┼──► Decision ──► ladder: judged ⇄ learned ⇄ deterministic
 5 wrapped tool ───────────────────────────┤        ▲                         │
 6 adopted pack ───────────────────────────┘        └──── evolution (7) ◄─────┘
```

| # | Path | Who writes the rule | Starts at | Can reach |
| --- | --- | --- | --- | --- |
| 1 | Authored | a person or agent | deterministic (`builtin`/`cel`), or documentation only | — |
| 2 | Authored from history | a person or agent, prompted by grouped history | deterministic or judged | deterministic |
| 3 | Prompted judge | a person or agent writes the judge prompt | judged (zero- or few-shot classifier) | learned, then deterministic |
| 4 | Mined | proposed by Lighthouse, ratified by a person or agent | learned (a model on mined labels) | deterministic |
| 5 | Wrapped tool | a person or agent binds an existing linter rule | deterministic (`command`/`rpc`) | — |
| 6 | Adopted pack | a pack author; the project enables it | whatever the pack ships | — |
| 7 | Evolution | proposed from memory, approved by a person or agent | an existing decision | narrower, wider, superseded, demoted |

### 1. Authored

A person, or an agent via MCP `decision_create`, writes the decision with a deterministic
check:
- a `cel` predicate over the code model;
- a standard op (`order`, `proximity`, `cycle`).

The decision is accepted once its `valid`/`invalid`/`fixed` examples pass. A decision
without `check:` is documentation only: it shows in docs and agent guidance, and the next
step for it is path 3.

Existing written conventions (a CONTRIBUTING file, ADR markdown, review guidelines) enter
here: an agent reads them and authors decisions. There is no markdown importer.

### 2. Authored from history

Like path 1, but the trigger is accumulated history instead of foresight.

1. Similar events are recorded as **signals**:
   - verdicts;
   - annotations;
   - repeated fixes of the same shape;
   - agent-session corrections ("use X, not Y").
2. Signals are grouped, in one of two ways:
   - automatically, by similarity (structural delta first, embeddings second; see [Grouping](#grouping));
   - by an agent that reviews the history (`decision_signals` lists them) and decides they are one decision.
3. A group that reaches support *k* is surfaced as a candidate.
4. A person or agent writes the decision. Group members become its examples and its `provenance`.

If the boundary is clear, the result is deterministic. Otherwise it starts as a judged check
(path 3) seeded with the group's labels.

### 3. Prompted judge

When the decision is real but no predicate captures it yet ("a function name states what it
returns, not how"), a person or agent writes a **judge prompt**:

```yaml
check:
  type: judged
  select: 'node.kind == "function" && node.changed'
  judge:
    prompt: Does the name state the result rather than the mechanism?
    shots: examples        # the decision's examples are the few-shot set
    output: {verdict: [violates, conforms], confidence: number}
```

- The judge is a zero- or few-shot System 1 classifier behind the `Judge` plugin kind, for example jev-class models. Without a Judge provider, the same prompt is served as an agent review task.
- Calibrated confidence decides what happens. A confident result is recorded as a judgment; an unsure one goes to an agent or person.
- Every judgment is a label. With enough weighted labels, a model (embedding + structural features + GBDT) is trained and takes over the confident region. This is the learned stage, gated by [Evaluation](#evaluation).
- When the learned boundary is expressible, the decision is promoted to a deterministic check.

### 4. Mined from change history

No one names the decision in advance. Lighthouse collects decision history from every change and proposes decisions.

1. **Collect** from commits (on `init` over a bounded window, then after each commit):
   - **Change direction.** In refactor-like commits (symbols moved or renamed, edges removed, tests unchanged), code before the change is a weak `violates` and code after is a weak `conforms`. A later revert flips the label.
   - **Prevalence.** In the current tree, the dominant shape within a cluster of similar code is a weak `conforms`; outliers are candidates. This works on a fresh clone with no history.
2. **Cluster.** See [Grouping](#grouping).
3. **Train.** One model per cluster on embedding + structural features (GBDT). It must beat the baselines.
4. **Propose.** A `proposed` decision with:
   - a requirement drafted from the delta signature;
   - examples generated from members (before → `invalid`, after → `valid`/`fixed`);
   - the model as its learned stage;
   - a judge prompt for the cases the model abstains on;
   - when the trees allow it, a deterministic draft.
5. **Ratify.** A person or agent accepts, edits or rejects it. A rejection suppresses the cluster until its evidence changes.

Mined labels are the weakest, so a mined model is capped at `warn`/`info` until human or
agent verdicts confirm it.

### 5. Wrapped tool

An existing linter rule (golangci-lint, clippy, ruff, semgrep, a script) becomes the
enforcement of a decision:
- through `check: {type: command}`, which uses exit codes and `path:line:col:` lines;
- or through `check: {type: rpc}`.

The decision adds what the tool lacks: intent, rationale, memory and verdicts. The rule stays
deterministic. Its precision is still measured from verdicts, so a noisy wrapped rule gets
narrowing proposals like any other.

### 6. Adopted pack

Bundled packs (`design`, `testing`) and third-party packs (catalogs shipped by plugins) are
decisions written elsewhere through paths 1–5. A project enables them through a preset and
tunes options per language. From then on they collect the project's own memory. Project
verdicts can narrow a pack decision locally through an override, without forking it.

### 7. Evolution of an existing decision

This does not create a rule; it changes one. Memory produces proposals:
- **Narrow:** rejected verdicts concentrate in one cluster, so `select` or the scope shrinks.
- **Widen:** confirmed but unflagged subjects come from audits or path 2 groups.
- **Demote:** precision drifts, so the check moves back one stage.
- **Supersede:** a new decision replaces the old one (`supersedes`). The old one stops enforcing, and its history is kept.

## Shared machinery

### Grouping

Used by paths 2 and 4.

1. **Structure first.** The key is the code-model delta signature, with identifiers abstracted:
   - edge kind added or removed;
   - symbol moved across an owner or module;
   - visibility changed;
   - test added;
   - order changed.

   Embeddings alone would cluster by topic, not by decision.
2. **Embedding second.** Within one signature, a local code embedding of the normalized hunk refines the clusters. Example: EmbeddingGemma 2, 270M parameters, 8k tokens, Matryoshka 768 → 256 dimensions. The embedding comes from an `Embedder` plugin and is cached by content hash.
3. **Support and consistency.** A cluster needs:
   - at least *k* members;
   - at least two commits or authors;
   - no reverse edits.

   Contradictions produce a conflict report, never a merge.
4. **Match before create.** A cluster matching an existing decision feeds path 7 instead of proposing a new decision.

### Enforcement cascade

A decision's `check:` selects one provider: `builtin`, `cel`, `command`, `rpc` or `judged`.
A `judged` check runs each selected subject through:

```text
 select candidates (CEL; usually the changed scope)
   ├─ deterministic stage, if present ─────────── decides
   ├─ learned stage: calibrated P(violates)
   │     ≥ upper → violates · ≤ lower → conforms · between → abstain
   └─ judge (prompt + shots), else an agent review task ── decides abstained cases
```

- A failing deterministic provider leaves the analysis incomplete (exit 3), never clean.
- A failing learned stage or judge skips with a notice.
- Severity is authored and does not change with the stage. An authored `error` is *definitive*: it needs no verdict, and only an annotation waives it.

### Learning

There is one model per decision, trained by an explicit `lighthouse learn train <decision>`.

| Feature group | Examples |
| --- | --- |
| Structural | code-model facts of the subject and delta: edge direction, depth, kind, ownership, graph distance, metrics |
| Semantic (compact) | similarity to the nearest `violates`/`conforms`, prototype similarity, cluster distance |
| Historical | the decision's precision, verdict counts of similar subjects |

- **No leakage.** Inputs are only what is known before judging. A judge's *reason* is never an input; it feeds grouping and path 7.
- **Out-of-fold similarity.** Neighbour features exclude the subject and its near-duplicates. Splits are by project, cluster and time.
- **Baselines first.** A model must beat the smoothed statistical baseline (Beta prior × cluster) and a kNN model; GBDT is used only when it wins. Raw embedding vectors are an ablation, not the default.
- **Label weight.** Human > agent > model judge > mined. A model trained mostly on judge or mined labels is capped at `warn`/`info`, and its agreement with human verdicts is reported separately.
- **Calibration.** Isotonic or Platt calibration on a holdout set comes before thresholds. Thresholds are recomputed on every retrain to meet the decision's precision target.
- **Reproducibility.** Every learned judgment records:
  - the decision's meaning version and check revision;
  - the feature version;
  - the embedder id and model version;
  - the model version.

  Models are local derived artifacts, rebuildable from `decisions.jsonl`, and are not committed.

### Decision memory

- **Verdict:** a judgment on a finding.
- **Judgment:** `violates` or `conforms` on a subject, including subjects nothing flagged.
- Both are appended to `.lighthouse/decisions.jsonl` (shared through git) and cached in SQLite.
- **Validity:**
  - They expire when the decision's *meaning version* (requirement, scope, severity, options) or the evidence changes.
  - A new *check revision* never expires them, so promotion keeps history.
- **Audit sampling:** a fraction of subjects that deterministic or learned stages left unflagged goes to the judge, for recall labels.
- **Exploration sampling:** a fixed fraction of confident learned calls still goes to the judge, to expose drift and misses.
- `lighthouse log compact` folds expired and superseded events; the raw history stays in git.

### Evaluation

The harness replays a candidate check (a new prompt, model or deterministic draft) against
recorded labels and all examples, through the CLI or MCP.

| Metric | Measured on |
| --- | --- |
| Precision, false-positive rate | verdicts and `violates` judgments |
| Recall, false-negative rate | `conforms`/`violates` judgments, audit samples |
| Coverage, abstention rate | the learned stage on the labelled set |
| Agreement | the previous stage on the same subjects |
| Examples | every `valid`/`invalid`/`fixed` example passes |

Each decision has an evaluation policy: its precision target and which error it tolerates.
A candidate that fails keeps the previous stage.

### Promotion

```text
 judged ──► learned ──► deterministic
   ▲           │             │
   └── demote ─┴── narrow ───┘
```

- **judged → learned:** enough weighted labels, and the model passes the gate and beats the baselines.
- **learned → deterministic:** the boundary can be expressed. A CEL or standard-op draft, from the model's tree paths or from an agent, must match the learned stage on the labelled set and pass all examples.
- Paths 3 and 4 climb this ladder; paths 1, 5 and 6 start at the top. Every move is a proposal with its evaluation report, approved explicitly.

## Status

| Part | State |
| --- | --- |
| Path 1, path 6, verdicts, annotations, `decisions.jsonl`, SQLite memory | done |
| Check providers (`command`/`rpc` for path 5, `judged` slot), meaning/check versions | in progress (2d-2) |
| Judgments on subjects, audit sampling, evaluation harness, log compaction | Phase 3 |
| Path 2 (signals, grouping, `decision_signals`), path 7 proposals | Phase 4 |
| Path 4 collection at `init` and on commit | Phase 4–5 |
| Judge providers (path 3 without an agent), learned stage, Embedder, cascade thresholds | Phase 7 (Embedder starts in Phase 3) |
