# Rule pipeline

This page covers how a decision gets a rule, and how that rule is judged, learned and refined.
Every rule ends in the same place: a `Decision` resource whose `check:` sits on one stage of
a ladder, plus a decision memory that moves it up or down. The [roadmap](roadmap.md) says
which parts exist today (see [Status](#status)).

Invariants that hold for every rule:
- **A decision is the source of truth.** Prompts, models, thresholds and examples derive from it and can be rebuilt.
- **Nothing enables itself.** Proposals, new models and promotions all need explicit approval.
- **Each decision is its own learning unit.** No single model judges every decision.
- **Precision before coverage.** A stage that is not confident passes the case on; it never guesses.
- **Every judgment becomes history.** It is recorded and becomes a label for the next round.
- **The goal is deterministic enforcement.** Judges and models are intermediate forms, not the product.
- **The core has no LLM API.** Judges, embedders and detectors are plugins, local first and opt-in when remote.

## How a rule comes to exist

Rules are not made through a fixed list of paths. Each rule sits at a point on four axes:
- where the decision came from;
- who stated it;
- how it is enforced;
- what evidence measures it.

It moves through a fixed set of **transitions**. A new situation is a new value on an axis,
never a new pipeline: the shared machinery below stays the same.

### Axis 1: origin, or what brings the decision up

| Origin | Examples | Evidence it yields |
| --- | --- | --- |
| Intent | a person or agent decides up front ("domain never imports infra") | authored examples |
| Existing artifact | a written convention, an ADR, a style guide, a review checklist, an external standard (OWASP, a language guide) | authored examples; an agent writes the decision, and there is no importer |
| Existing tool rule | a golangci-lint, clippy, ruff or semgrep rule, or a script | the tool's findings, then verdicts on them |
| Recorded history | verdicts, annotations, repeated fixes of the same shape, review comments, agent-session corrections ("use X, not Y") | signals with polarity and actor |
| Incident | a bug fix, postmortem, CI break or revert: "this must not happen again" | the incident site is `violates`, the fix is `conforms` |
| Repository mining | change direction in refactor-like commits; prevalence in the current tree | weak labels: before/outlier is `violates`, after/dominant is `conforms` |
| Snapshot | current metrics frozen as a budget ("complexity of this module must not grow") | the baseline itself; the decision is a ratchet |
| Another project | an organisation catalog, a third-party pack, a decision proven elsewhere | the other project's examples; its labels are not imported, only the decision |

### Axis 2: author, or who states the decision

- **A person** or **an agent** writes it (MCP `decision_create`); its status is `accepted` once its examples pass.
- **Lighthouse** proposes it (`status: proposed`) from grouped signals or mining, with provenance and generated examples. Lighthouse never accepts; a person or agent ratifies, edits or rejects. A rejection suppresses the same cluster until its evidence changes.

### Axis 3: form, or how it is enforced

| Form | Needs | Provider |
| --- | --- | --- |
| documentation | a statement | none; shown in docs and agent guidance |
| judged | a statement, `select`, and a judge prompt (examples as shots) | Judge plugin (e.g. a jev-class zero/few-shot classifier), else an agent review task |
| learned | enough weighted labels, and passing the [evaluation](#evaluation) gate | Detector plugin (embedding + structural features, GBDT when it beats the baselines) |
| deterministic | a predicate and passing examples | `builtin` standard ops, `cel`, `command`, `rpc` |

A judged decision may hold all three stages at once. The deterministic part decides what it
can, the learned stage decides where it is confident, and the judge decides the rest (see the
[cascade](#enforcement-cascade)).

```yaml
check:
  type: judged
  select: 'node.kind == "function" && node.changed'
  judge:
    prompt: Does the name state the result rather than the mechanism?
    shots: examples        # the decision's examples are the few-shot set
    output: {verdict: [violates, conforms], confidence: number}
```

### Axis 4: labels, or what trains and measures it

From strongest to weakest:
1. human verdicts and judgments;
2. agent verdicts and judgments;
3. annotations, which are intentional exceptions;
4. incident sites;
5. model-judge judgments;
6. mined labels.

Audit and exploration samples are drawn to cover recall and drift. A decision whose labels
are mostly weak is capped at `warn`/`info` until stronger labels confirm it.

Two more properties are fixed per decision. They don't change how it is made:
- **Domain:** code today; later documents and agent sessions.
- **Reach:** project, organisation pack, or bundled.

### Transitions

| Transition | Trigger | Gate |
| --- | --- | --- |
| create | any origin | examples pass; accepted or ratified |
| promote: judged → learned → deterministic | enough weighted labels; a draft predicate from tree paths or an agent | evaluation: precision, recall, agreement with the previous stage, examples |
| demote | precision drifts; exploration samples disagree | automatic proposal, explicit approval |
| narrow / widen | rejections concentrated in a cluster; confirmed but unflagged subjects | replay on recorded labels |
| split / merge | one decision with two disagreeing clusters; two decisions flagging the same subjects | replay on both; verdicts are remapped by subject |
| supersede | a new decision replaces an old one (`supersedes`) | the old one stops enforcing; history is kept |
| deprecate | no findings or judgments for a long time; its scope no longer exists | explicit |
| upstream / adopt | a project decision is moved into an organisation pack, or a pack decision is enabled elsewhere | in the new project the decision restarts at its authored form; models are retrained and thresholds recalibrated on local labels |

Every transition except creation by a person or agent is a **proposal** with an evaluation
report. Meaning-preserving transitions (promote, demote, narrow within the same requirement)
keep verdicts, because only the check revision changes. Transitions that change the meaning
version start a new verdict history.

### Common recipes

| Recipe | Origin | Author | Starts as | Typically becomes |
| --- | --- | --- | --- | --- |
| Write a rule | intent, existing artifact | person or agent | deterministic or documentation | — |
| Rule from accumulated history | recorded history | person or agent (grouping automatic, or the agent reviews `decision_signals`) | deterministic or judged | deterministic |
| Prompted judge | intent, existing artifact | person or agent writes the prompt | judged | learned, then deterministic |
| Never again | incident | person or agent | deterministic if expressible, else judged | deterministic |
| Mined proposal | repository mining | Lighthouse, then ratified | learned, plus a judge prompt for abstentions | deterministic |
| Wrapped tool | existing tool rule | person or agent | deterministic (`command`/`rpc`) | narrowed by verdicts |
| Budget | snapshot | person or agent | deterministic ratchet | tightened over time |
| Adopted decision | another project | pack author; the project enables it | the pack's form | narrowed locally through overrides |

**Mined proposals in detail.**
1. **Collect.** On `init` over a bounded window, then after each commit. Only refactor-like commits are labelled (symbols moved or renamed, edges removed, tests unchanged); a later revert flips the label. Prevalence needs no history.
2. **[Group](#grouping).**
3. **Train** one model per cluster; it must beat the baselines.
4. **Propose.** A requirement drafted from the delta signature; examples from members (before → `invalid`, after → `valid`/`fixed`); the model as the learned stage; a judge prompt for abstentions; and a deterministic draft when the trees allow it.

### Open questions

- **Incident linking.** How a bug fix is recognised as design-relevant without an agent: commit message, linked issue, or test added with the fix?
- **Split detection.** What disagreement between clusters inside one decision is enough to propose a split?
- **Deprecation.** Is silence evidence that a decision is obsolete, or that it works?
- **Cross-project labels.** Should labels ever travel with an upstreamed decision, given privacy and differing codebases? Default: no.
- **Session domain.** Decisions about agent actions reuse these axes, but their subjects are tool calls, not code. Their labels and latency budget are still open.


## Shared machinery

### Grouping

Used for recorded history and repository mining.

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
4. **Match before create.** A cluster matching an existing decision becomes a narrow or widen proposal instead of a new decision.

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

- **No leakage.** Inputs are only what is known before judging. A judge's *reason* is never an input; it feeds grouping, narrowing and widening.
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
- Every move is a proposal with its evaluation report, approved explicitly.

## Status

| Part | State |
| --- | --- |
| Authored decisions, adopted packs, verdicts, annotations, `decisions.jsonl`, SQLite memory | done |
| Check providers (`command`/`rpc` for wrapped tools, `judged` with `select` and `judge` reserved), meaning/check versions | in progress (2d-2) |
| Judgments on subjects, audit sampling, evaluation harness, log compaction, snapshot budgets (baseline and ratchet) | Phase 3 |
| Signals, grouping, `decision_signals`, Lighthouse proposals, narrow/widen/split/merge/demote proposals | Phase 4 |
| Repository mining at `init` and on commit | Phase 4–5 |
| Judge providers, learned stage, Embedder, cascade thresholds | Phase 7 (Embedder starts in Phase 3) |
