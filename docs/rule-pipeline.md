# Rule pipeline

Lighthouse is not a fixed rule pipeline. It is a decision evolution system: a **decision** is
the source of truth, and Lighthouse repeats **evidence → check → judgment → evaluation →
revision** on it. This page defines that model. The methods behind learned checks and
repository mining are in [learning.md](learning.md). The [roadmap](roadmap.md) says what
exists today (see [Status](#status)).

## Primitives

```text
 Decision ─┬─ Meaning     requirement, scope, severity, options      → meaning version
           ├─ Check       deterministic check, judge, learned stage  → check revision
           ├─ Evidence    signals and snapshots with origin, author and strength
           ├─ Judgment*   verdicts on findings, judgments on subjects
           └─ Revision*   proposed and approved changes, each with its evaluation
```

| Primitive | Is | Is not |
| --- | --- | --- |
| Decision | the identity: what was decided and why (`Decision` resource) | a state machine; a decision has no `stage` field |
| Meaning | what the decision demands; its hash is the **meaning version** | how it is checked |
| Check | how the meaning is enforced now; its hash is the **check revision** | part of the meaning |
| Evidence | anything that bears on the decision: examples, signals, snapshots, each with provenance | a conclusion |
| Judgment | a recorded conclusion about one subject or finding | a training rule; it is data |
| Revision | a change to a decision, with its evaluation and approval | a mutation in place |

**Meaning version and check revision are the central axis.**
- A judgment holds while the meaning version and its evidence are unchanged.
- A new check revision (a better predicate, a retrained model, a narrower selector) never expires judgments. This is what lets a check improve without losing its history.
- A new meaning version starts a fresh judgment history.

## Decision

A decision is a `Decision` resource committed with the catalog (see
[architecture](architecture.md)). Besides its meaning, it carries ADR fields:
- `title`, `intent`, `rationale`, `consequences`;
- `status` (`proposed`, `accepted`, `superseded`, `deprecated`), `supersedes`;
- `provenance`, the evidence that produced it.

## Check

A decision's check has up to three parts, and which parts are present decides how a subject
is routed. No `type` says which one is primary.

```yaml
check:            # deterministic part: builtin | cel | command | rpc
  type: cel
  select: function
  where: metrics(node).cognitive > options.cognitive
judge:            # judge part: candidates and a prompt
  select: 'node.kind == "function" && node.changed'
  prompt: Does the name state the result rather than the mechanism?
  shots: examples # the decision's examples are the few-shot set
# learned part: not authored; an approved revision attaches a trained model
```

| Parts present | Behaviour |
| --- | --- |
| none | documentation: appears in docs and agent guidance, produces no findings |
| `check` | deterministic enforcement |
| `judge` | each candidate is judged |
| `judge` + learned | the model decides where it is confident; the judge decides the rest |
| `check` + `judge` (+ learned) | the deterministic part decides what it covers; the remaining candidates are routed as above |

### Routing, not maturity

```text
 candidates
   ├─ deterministic   cheap, certain        ── decides what it covers
   ├─ learned         cheap, probabilistic  ── violates ≥ upper · conforms ≤ lower · else abstains
   └─ judge           expensive, semantic   ── decides the rest (Judge plugin, else an agent review task)
```

- The order is about **cost and confidence**, not quality. A learned model approximates a boundary that has no faithful predicate; it is not a weaker deterministic rule.
- Deterministic enforcement is the **preferred** endpoint when the boundary can be expressed faithfully. It is not a required one: some decisions ("names state the result, not the mechanism") stay judged for good, and that is a normal state.
- A failing deterministic part leaves the analysis incomplete (exit 3), never clean. A failing learned part or judge skips with a notice.
- Severity is part of the meaning, not of the routing. An authored `error` is *definitive*: it needs no verdict, and only an annotation waives it.

## Evidence

Every piece of evidence records:
- **Origin:** what brought it up.
- **Author:** who stated it.
- **Strength:** how far it can be trusted as a label.

These are properties of evidence, not kinds of decision. Any origin combines with any author.

| Origin | Examples | Typical labels |
| --- | --- | --- |
| Intent | a person or agent decides up front | authored examples |
| Existing artifact | a convention, an ADR, a style guide, an external standard; an agent writes the decision (no importer) | authored examples |
| Existing tool rule | a golangci-lint, clippy, ruff or semgrep rule, or a script, wrapped by `command`/`rpc` | the tool's findings, then verdicts |
| Recorded history | verdicts, annotations, repeated fixes of one shape, review comments, agent-session corrections | signals with polarity |
| Incident | a bug fix, postmortem, CI break or revert | incident site `violates`, fix `conforms` |
| Repository mining | change direction in refactor-like commits; prevalence in the tree ([learning.md](learning.md)) | weak `violates`/`conforms` |
| Snapshot | current metrics frozen as a budget | the baseline; the decision is a ratchet |
| Another project | an organisation catalog, a third-party pack | that project's examples (its labels never travel) |

- **Author:** a person, an agent, or Lighthouse. Lighthouse only *proposes* (`status: proposed`); a person or agent ratifies.
- **Strength**, from strongest to weakest:
  1. human;
  2. agent;
  3. annotation;
  4. incident site;
  5. model judge;
  6. mined.

  A check whose evidence is mostly weak is capped at `warn`/`info` until stronger labels confirm it.

Mining, grouping and similarity are **evidence producers**. The model above holds without
them: intent → decision is a complete loop.

## Judgment

Two records with different meanings:

| Record | About | Values | Means |
| --- | --- | --- | --- |
| **Judgment** | a subject (a symbol, file, edge…) | `violates`, `conforms` | ground truth for the meaning; the unit of labels |
| **Verdict** | a finding produced by a check | `confirmed`, `rejected` (with a reason), `deferred` | a reviewer's response to the check's output |

- A verdict implies a judgment only for some reasons:
  - `confirmed` implies `violates`;
  - `rejected: false-positive` implies `conforms`;
  - `rejected: not-worth-fixing`, `intentional-exception` or `scope-too-broad` imply nothing about the subject. They are evidence about the check's scope or the decision's value.
- Judgments also exist where no finding does:
  - **audit samples**, which the deterministic or learned parts left unflagged;
  - **exploration samples**, a fixed fraction of confident learned calls re-judged;
  - judge results on candidates.

  These give recall and drift, not only precision.
- Everything is appended to `.lighthouse/decisions.jsonl` (shared through git) and cached in SQLite. `lighthouse log compact` folds expired and superseded records; the raw history stays in git.

## Evaluation

A candidate check revision is replayed against the decision's judgments and examples, through
the CLI or MCP:

| Metric | Measured on |
| --- | --- |
| Precision, false-positive rate | `violates` judgments and confirmed verdicts |
| Recall, false-negative rate | `conforms`/`violates` judgments, audit samples |
| Coverage, abstention rate | the learned part |
| Agreement | the current revision, on the same subjects |
| Examples | every `valid`/`invalid`/`fixed` example passes |

Each decision has an evaluation policy: its precision target and which error it tolerates.

## Revision

Every change to a decision is a revision: `{decision, parent, meaning version, check revision,
evaluation, approval}`.
- Authored edits to the YAML are revisions whose history is git.
- Derived parts (a trained model, thresholds) and approvals are `Revision` records in `decisions.jsonl`.
- Lighthouse proposes revisions; it never applies one without approval.

Revisions fall into four families:

| Family | Changes | Kinds | Judgments |
| --- | --- | --- | --- |
| Enforcement refinement | the check, not the meaning | add or replace a deterministic part, attach or retrain a learned part, drop a part (demote), narrow or widen a selector | kept |
| Identity restructuring | the decision's boundary | split one decision into two, merge two into one | remapped by subject, then re-evaluated |
| Lifecycle | whether it enforces | accept, supersede, deprecate | kept as history |
| Distribution | where it lives | upstream to an organisation pack, adopt from one | not transferred; models retrained and thresholds recalibrated on local labels |

"Promotion" (judged → learned → deterministic) is not a state change. It is a sequence of
refinement revisions, each of which must pass evaluation against the current one.

## Open questions

- **Incident linking.** How a bug fix is recognised as design-relevant without an agent: commit message, linked issue, or test added with the fix?
- **Split detection.** What disagreement between clusters inside one decision justifies a split proposal?
- **Deprecation.** Is silence evidence that a decision is obsolete, or that it works?
- **Cross-project labels.** Should labels ever travel with an upstreamed decision? Default: no.
- **Session domain.** Decisions about agent actions reuse this model, but their subjects are tool calls. Their labels and latency budget are still open.

## Status

| Part | State |
| --- | --- |
| Decisions, packs, verdicts, annotations, `decisions.jsonl`, SQLite memory | done |
| `check` providers, reserved `judge` block, meaning version and check revision | in progress (2d-2) |
| Judgments on subjects, audit sampling, evaluation, `Revision` records, log compaction, snapshot budgets | Phase 3 |
| Evidence producers (signals, grouping, mining) and refinement/restructuring proposals | Phase 4 (mining at `init` in Phase 5) |
| Judge providers, learned part, Embedder | Phase 7 (Embedder starts in Phase 3) |
