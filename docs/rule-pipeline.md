# Rule pipeline

This page covers how Lighthouse produces and refines the rules that enforce decisions: where
candidate decisions come from, how they become checks, how judgments are recorded, and how
a check moves from judgment to a learned model to a deterministic rule. The
[roadmap](roadmap.md) says which stages exist today (see [Status](#status)).

```text
 sources ─► signals ─► grouping ─► proposed decision ─► ratification
                                                            │
     ┌──────────────────────────────────────────────────────┘
     ▼
 enforcement: select ─► deterministic? ─► learned (abstains) ─► judge
     │
     ▼
 findings, verdicts, judgments ─► decision memory ─► evaluation ─► promotion / narrowing
     ▲                                                              │
     └──────────────────────────────────────────────────────────────┘
```

Invariants that hold at every stage:
- **A decision is the source of truth.** Checks, models, thresholds and examples derive from it and can be rebuilt.
- **Nothing enables itself.** Proposals, new models and promotions all need explicit approval.
- **Each decision is its own learning unit.** No single model judges every decision.
- **Precision before coverage.** A stage that is not confident passes the case on; it never guesses.
- **Every judgment becomes history.** It is recorded and becomes a label for the next round.
- **The goal is deterministic enforcement.** Models are an intermediate form, not the product.
- **The core has no LLM API.** Judges, embedders and detectors are plugins, local first and opt-in when remote.

## 1. Sources

A source turns something that happened into **signals**. A signal records:
- a statement or structural change;
- the subjects it concerns;
- a polarity (`violates`, `conforms` or neutral);
- the actor (human, agent, model or mined);
- an evidence snapshot, a commit and a time.

| Source | Produces | Polarity from |
| --- | --- | --- |
| Authoring | a decision written by a person, or by an agent via MCP `decision_create` | the author |
| Verdicts | a verdict on a finding (`confirmed`/`rejected`, with a structured reason) | the reviewer |
| Annotations | `lighthouse:allow <decision> -- <reason>` in code | an intentional exception |
| Fix outcomes | a fix applied, rolled back or declined | the outcome |
| Decision edits | narrowing, widening, superseding | the edit |
| Repository history | design edits mined from commits | change direction: before `violates`, after `conforms` |
| Conventions | the shape of the current tree | prevalence: dominant shape `conforms`, outliers are candidates |
| Agent sessions (later) | corrections during agent work ("use X, not Y") | the correction |

**Repository history** runs on `lighthouse init` over a bounded window (the last N commits)
and then incrementally after each commit.
- Only refactor-like commits are labelled: symbols moved or renamed, edges removed, tests
  unchanged, behaviour-neutral. Feature commits are not.
- A later revert flips the label.

**Conventions** need no history, so `init` works on a fresh clone.

## 2. Grouping

Signals are grouped into candidate decisions.

1. **Structure first.** The key is the code-model delta signature, with identifiers abstracted:
   - edge kind added or removed;
   - symbol moved across an owner or module;
   - visibility changed;
   - test added;
   - order changed.

   Embeddings alone would cluster by topic ("payment code"), not by decision ("a domain package stopped importing infrastructure").
2. **Embedding second.** Within one signature, a local code embedding of the normalized hunk refines the clusters. Example: EmbeddingGemma 2, 270M parameters, 8k tokens, Matryoshka 768 → 256 dimensions. The embedding comes from an `Embedder` plugin and is cached by content hash.
3. **Support and consistency.** A cluster needs:
   - at least *k* members;
   - at least two commits or authors;
   - no reverse edits.

   Contradicting signals produce a conflict report, never a merge.
4. **Match before create.** A cluster that matches an existing decision strengthens, narrows or widens that decision instead of proposing a new one.

## 3. Proposal

A cluster becomes a `Decision` with `status: proposed`:
- `provenance`: the member signals (commits, subjects, verdicts).
- `requirement`: drafted from the delta signature; an agent may reword it.
- `examples`: generated from the members. Before-code becomes `invalid`, after-code becomes `valid` or `fixed`.
- `check`: a `judged` check with a `select` covering the cluster's subject shape. Also attached, when available:
  - a learned model (section 6);
  - a deterministic draft (CEL or standard ops) from the model's tree paths or from an agent.
- Confidence and support, so proposals can be ranked.

## 4. Ratification

A person or agent accepts, edits or rejects the proposal.
- Accepting sets `status: accepted`; the decision is committed with the catalog.
- A rejection is a signal too: the same cluster is not proposed again until its evidence changes.
- Ratified mined decisions start at `warn` or `info`.
- An authored `error` is *definitive*: it needs no verdict, and only an annotation waives it.

## 5. Enforcement

A decision's `check:` selects one provider: `builtin` (standard ops `order`, `proximity`,
`cycle`), `cel`, `command`, `rpc` or `judged`.

A `judged` check runs each selected subject through a cascade:

```text
 select candidates (CEL; usually the changed scope)
   │
   ├─ deterministic stage (if the decision has one) ── decides
   ├─ learned stage: calibrated P(violates)
   │     ≥ upper  → violates
   │     ≤ lower  → conforms
   │     between  → abstain
   └─ judge (Judge provider, else an agent review task) ── decides abstained cases
```

- The learned stage only handles the region where it is confident. Its coverage grows as it improves.
- A failing deterministic provider leaves the analysis incomplete (exit 3), never clean.
- A failing learned stage or judge skips with a notice.

## 6. Learning

There is one model per decision, trained by an explicit `lighthouse learn train <decision>`.

| Feature group | Examples |
| --- | --- |
| Structural | code-model facts of the subject and delta: edge direction, depth, kind, ownership, graph distance, metrics |
| Semantic (compact) | similarity to the nearest `violates` and `conforms`, prototype similarity, cluster distance |
| Historical | the decision's precision, verdict counts of similar subjects |

Rules that keep the model honest:
- **No leakage.** Inputs are only what is known before judging. The judge's *reason* is never an input; it feeds grouping and narrowing.
- **Out-of-fold similarity.** Neighbour and prototype features exclude the subject and its near-duplicates. Splits are by project, cluster and time.
- **Baselines first.** A model must beat the smoothed statistical baseline (Beta prior × cluster) and a kNN model. Gradient-boosted trees (LightGBM) are used only when they win. Raw embedding vectors are an ablation, not the default.
- **Label weight.** Human > agent > model judge > mined. A model trained mostly on model-judge or mined labels is capped at `warn`/`info`, and its agreement with human verdicts is reported separately.
- **Calibration.** Isotonic or Platt calibration on a holdout set comes before thresholds. Thresholds are recomputed on every retrain to meet the decision's precision target.
- **Reproducibility.** Every learned judgment records:
  - the decision's meaning version and check revision;
  - the feature version;
  - the embedder id and model version;
  - the model version.

  Models are local derived artifacts, rebuildable from `decisions.jsonl`, and are not committed.

## 7. Decision memory

- **Verdict:** a judgment on a finding.
- **Judgment:** `violates` or `conforms` on a subject, including subjects nothing flagged.
- Both are appended to `.lighthouse/decisions.jsonl` (shared through git) and cached in SQLite.
- **Validity:**
  - Verdicts and judgments expire when the decision's *meaning version* (requirement, scope, severity, options) or the evidence changes.
  - A new *check revision* never expires them, so promoting a check keeps its history.
- **Audit sampling:** a fraction of subjects that deterministic or learned stages left unflagged goes to the judge. This supplies recall labels.
- **Exploration sampling:** a fixed fraction of the learned stage's confident calls, on both sides, still goes to the judge. This exposes drift and misses inside the automated region.
- `lighthouse log compact` folds expired and superseded events; the raw history stays in git.

## 8. Evaluation

The evaluation harness replays a candidate check against recorded labels and all examples. It is used by people and agents alike, through the CLI and MCP.

| Metric | Measured on |
| --- | --- |
| Precision, false-positive rate | verdicts and `violates` judgments |
| Recall, false-negative rate | `conforms`/`violates` judgments, audit samples |
| Coverage, abstention rate | the learned stage on the labelled set |
| Agreement | the previous stage on the same subjects |
| Examples | every `valid`/`invalid`/`fixed` example passes |

Each decision has an evaluation policy: its precision target and which kind of error it can
tolerate. A candidate that fails keeps the previous stage in place.

## 9. Promotion and narrowing

```text
 judged ──► learned ──► deterministic
   ▲           │             │
   └── demote ─┴── narrow ───┘   (when precision drifts or verdicts disagree)
```

- **judged → learned:** enough weighted labels, and the model passes the gate and beats the baselines.
- **learned → deterministic:** the decision boundary can be expressed. A CEL or standard-op draft, from tree paths or an agent, must match the learned stage on the labelled set and pass all examples.
- **Narrowing and demotion:** rejected verdicts concentrated in one cluster lead to a proposal that narrows `select` or the scope. A noisy check is demoted to the previous stage.
- Every move is a proposal with its evaluation report, approved explicitly. Severity does not change with the stage. The stage changes the provider, not what the decision demands.

## Status

| Stage | State |
| --- | --- |
| Authoring, verdicts, annotations, `decisions.jsonl`, SQLite memory | done |
| Check providers, standard ops, severity, meaning/check versions | in progress (2d-2) |
| Judgments on subjects, audit sampling, evaluation harness, log compaction | Phase 3 |
| Signals, grouping, proposals, repository-history and convention sources | Phase 4 |
| `init` with mined starter decisions | Phase 5 |
| Embedder, Detector (learned stage), Judge providers, cascade thresholds | Phase 7 (Embedder starts in Phase 3) |
