# Rule pipeline

Lighthouse is not a fixed rule pipeline. It is a decision evolution system: a **decision** is
the source of truth, and Lighthouse repeats **evidence → check → judgment → revision** on it.
This page defines that model with as few concepts as possible. The methods behind trained
classifiers and repository mining are in [learning.md](learning.md). The [roadmap](roadmap.md)
says what exists today (see [Status](#status)).

## Concepts

```text
 Decision ─┬─ meaning       what it demands             → meaning version
           └─ check         how it is evaluated now     → check revision
 Evidence                   what was observed
 Judgment                   pass | fail (conformance) or notApplicable (applicability)
 Suppression                why a fail is not acted on
 Revision                   an approved change to a decision, with its evaluation
```

| Concept | Is | Absorbs |
| --- | --- | --- |
| Decision | what was decided and why, and how it is evaluated (`Decision` resource) | rule, pattern, proposal (a decision with `status: proposed`) |
| Evidence | what was observed, with provenance; carries no result | signal, snapshot, incident, mined edit |
| Judgment | a recorded result on one subject for one meaning version, drawn from evidence: conformance (`pass`/`fail`) or applicability (`notApplicable`) | verdict, label, example (an authored judgment on an example subject), audit and exploration samples |
| Suppression | a recorded reason not to act on a `fail` (SARIF suppression) | annotation (`inSource`), intentional exception and won't-fix (`external`) |
| Revision | a change to a decision, with its evaluation and approval | promotion, demotion, narrowing, widening, model attachment |

**Meaning version and check revision are the central axis.**
- A judgment belongs to the meaning version it was made under. It holds while that version and its evidence are unchanged.
- A new check revision (a better predicate, a retrained model, a narrower selector) never expires judgments, so a check improves without losing its history.
- A new meaning version never silently inherits judgments.
  - Older judgments may be reused only as evidence.
  - They become judgments for the new version only after revalidation: by a model, a person or an agent, or in bulk as one explicit, recorded approval that names the actor.

## Decision

A decision is a `Decision` resource committed with the catalog (see
[architecture](architecture.md)). Its fields, and nothing more:

| Group | Fields |
| --- | --- |
| Meaning (hashed into the meaning version) | `requirement` (RFC 2119 wording, exceptions included), `scope`, `severity`, `options`, `languages.<id>.options` |
| Record (ADR) | `title`, `context` (why), `consequences`, `status`, `supersedes` |
| Enforcement | `check` (optional; none = documentation), `fix` (optional) |
| Examples | `examples`: authored judgments (`valid`, `invalid`, `fixed`) |
| Provenance | `provenance`: PROV `wasDerivedFrom` (papers, source documents, issues, signals) |

- `status` follows MADR: `proposed`, `accepted`, `rejected`, `deprecated`, `superseded`.
- `supersedes` is a list: one decision superseding several is a merge; several superseding one is a split.
- Grouping (pack, section, preset membership) is `metadata.labels`, not spec fields.

Severity belongs to the meaning. An authored `error` is *definitive*: it needs no judgment,
and only a suppression waives it.

## Check

A check answers one question: *how is this decision evaluated on a subject now?* It has two
parts, and each answers a different question:

| Part | Question | Result |
| --- | --- | --- |
| selector (`select`, or the provider's own scope) | does the decision apply to this subject? | applicable, or `notApplicable` |
| evaluator | does the subject conform? | `pass` or `fail` |

A check is one provider, configured like every other provider in Lighthouse (fix,
formatter):
- **`type`** names where evaluation happens: `builtin` (a standard op in process), `cel` (an expression), `command` (a process), `rpc` (a plugin process) or `model` (the bound classification model).
- **Capabilities** in the manifest say how it behaves: `deterministic`, `abstains`, `cost`.

The engine keys every behaviour off capabilities, never off type, op or model names. There
are no profiles or tiers beyond that.

**Results and execution errors are different things.** A provider that runs and finds a
violation produces a `fail` result, which is a normal finding. An **execution error** (a
crash, a timeout, unparsable output) is not a result:
- From a deterministic provider, it leaves the analysis incomplete (exit 3). Nothing it would have covered can be called clean.
- From a non-deterministic provider, it skips the affected subjects with a notice; they stay `review`.

Execution errors are reported as SARIF tool execution notifications.

### Models

```yaml
check:
  type: model
```

That is the whole check for a decision the model should evaluate:
- **Question:** the decision's `requirement`. An optional `prompt` replaces it when the requirement reads badly as a question.
- **Shots:** the decision's examples.
- **Candidates:** the subjects in the decision's `scope`. An optional `select` (CEL) narrows them.
- **Output:** fixed by the task, so there is nothing to configure.

Models are one plugin kind, `Model`, with one of two tasks,
because each task has its own call shape:

| Task | Call |
| --- | --- |
| `classification` | subject + decision (requirement or prompt, examples) → `pass`/`fail` with a confidence, or abstain |
| `embedding` | texts → vectors |

How a model answers is its own business. An LLM, a jev-class System 1 model, or a gradient-
boosted classifier trained on the decision's judgments all serve `classification` the same
way.

**A decision never names a model.** Decisions are shared through git and packs, while which
models exist (local or remote, allowed or not, at what cost) depends on the machine and the
project. A decision says what to ask; bindings say who answers. Bindings resolve from the
most specific layer to the least, and an **abstention falls through** to the next one:

| Layer | Where | Set by |
| --- | --- | --- |
| 1. Trained for this decision | a `Revision` record in `decisions.jsonl` naming the model id and version; the artifact is local and rebuildable | Lighthouse trains on request (`lighthouse learn train <decision>`); a person or agent approves the revision |
| 2. Project, per decision | `[rules."<id>"] model = "…"` | the project |
| 3. Project default | `[models] classification = "…"`, `embedding = "…"` | the project |
| 4. Agent | a review task | always available; how `type: model` is served today |

```toml
[models]
classification = "my-llm"
embedding = "embeddinggemma"

[rules."design/naming-result"]
model = "local-small"            # cheaper model for this decision only
```

- A cheap model that abstains in front of an expensive one is just layer 1 or 2 over layer 3. It is not a separate mechanism.
- If a layer-1 artifact is missing on a machine (a fresh clone, say), it is rebuilt from `decisions.jsonl`, or skipped with a notice and the case falls through.
- `embedding` is bound only per project. Classifiers trained on embeddings record the embedding model in their version tuple, and changing the embedding model invalidates them.
- The order is about **cost and confidence**, not quality. A trained classifier approximates a boundary that has no faithful predicate; it is not a weaker deterministic rule.
- A deterministic check decides every subject it selects. Partial deterministic coverage of a model-evaluated decision is expressed by narrowing its `select`, not by stacking providers.
- Deterministic evaluation is the **preferred** endpoint when the boundary can be expressed faithfully. It is not a required one: some decisions stay model-evaluated for good.

## Evidence

Evidence is what was observed. It carries no result. Every record (evidence, judgment,
suppression, revision, decision) has the same provenance, in W3C PROV-O terms:
- `wasAttributedTo`: the person or software agent (an agent, a model, a mining rule, Lighthouse);
- `wasDerivedFrom`: what it came from;
- `generatedAtTime`: when it was made.

There is no separate author, producer or reviewer field. The origins below are examples of
`wasDerivedFrom`, not a type.

| Origin | Examples | Judgments that may be drawn |
| --- | --- | --- |
| Intent | a person or agent decides up front | example judgments |
| Existing artifact | a convention, an ADR, a style guide, an external standard; an agent writes the decision (no importer) | example judgments |
| Existing tool rule | a golangci-lint, clippy, ruff or semgrep rule, or a script, wrapped by `command`/`rpc` | the tool's `fail` results, then judgments on them |
| Recorded history | judgments, suppressions, repeated fixes of one shape, review comments, agent-session corrections | judgments with the same polarity |
| Incident | a bug fix, postmortem, CI break or revert | the incident site `fail`, the fixed code `pass` |
| Repository mining | change direction in refactor-like commits; prevalence in the tree ([learning.md](learning.md)) | weak `fail` (before, outlier) and `pass` (after, dominant) |
| Snapshot | current metrics frozen as a budget | the baseline; the decision is a ratchet |
| Another project | an organisation catalog, a third-party pack | its examples (its other judgments never travel) |

- Lighthouse only *proposes* (`status: proposed`); a decision attributed to Lighthouse becomes `accepted` only by a person or agent.
- **Judgment strength** follows `wasAttributedTo`: person > agent > model > mining rule. A model's own outputs never train that model. A stronger judgment on the same subject and meaning version supersedes a weaker one. A check whose judgments are mostly weak is capped at `warn`/`info`.

Mining, grouping and similarity are evidence sources. The model holds without them:
intent → decision is a complete loop.

## Judgment and suppression

A **judgment** is a result on one subject, and it labels one of the check's two parts:
- `pass` and `fail` are **conformance** labels; they evaluate the evaluator;
- `notApplicable` is an **applicability** label; it evaluates the selector.

The values share one field because SARIF `result.kind` does the same, but they are never
mixed in metrics. A judgment is a recorded label, not ground truth.

What used to be a verdict on a finding is now one judgment, plus a suppression when the
finding is right but will not be acted on:

| Reviewer says | Judgment | Suppression |
| --- | --- | --- |
| the finding is right (`fixed`, `accepted-debt`) | `fail` | — |
| false positive | `pass` | — |
| the decision should not apply here (scope too broad) | `notApplicable` | — |
| intentional exception, project allows it | `fail` | `external`, with a justification |
| won't fix | `fail` | `external`, justification `wont-fix` |
| `lighthouse-disable-next-line <decision> -- <reason>` (or `-line`, or a `lighthouse-disable` range) in code | — | `inSource`, with the reason |
| not sure yet | — (stays `review`) | — |

What a run **reports** uses SARIF results with their standard meaning:
- `fail` is a finding. A suppressed `fail` stays in SARIF with its suppression.
- `review` is a case nobody has decided yet.
- `notApplicable` is a subject outside the selector, or judged so.

Judgments also exist where no finding does. **Sampling** re-judges a fixed fraction of
subjects a check already decided, unflagged ones and confident ones alike, with a stronger
attribution. This gives recall and drift, not only precision.

A finding's identity (its fingerprint) is seeded by the decision's uid, so verdicts survive
a rename; see [architecture](architecture.md#fingerprints).

Judgments and suppressions are appended to `.lighthouse/decisions.jsonl` (shared through git)
and cached in SQLite. `lighthouse log compact` folds expired and superseded records; the raw
history stays in git.

## Revision

A revision is a change to a decision: `{decision, parent, meaningVersion, checkRevision,
change, evaluation, approval}`.
- Authored edits to the YAML are revisions whose history is git.
- Decision-level model bindings, thresholds and approvals are `Revision` records in `decisions.jsonl`.
- Lighthouse proposes revisions; it never applies one without approval.

**`change` is derived, never declared.** The classification is inspired by Semantic
Versioning but is not SemVer: it says which version of the decision moved.

| `change` | When | Examples | Evaluation | Judgments |
| --- | --- | --- | --- | --- |
| `major` | the meaning version changed | a stricter requirement, a new option default | required | reused only as evidence, revalidated |
| `minor` | only the check revision changed | replace `type: model` with a `cel` check, bind or retrain a decision-level model, narrow or widen a selector | required, against the current revision | kept |
| `patch` | neither changed | wording, rationale | none | kept |

**Evaluation** replays the candidate against the decision's judgments, examples included:

| Metric | Measured on |
| --- | --- |
| Precision, false-positive rate | conformance judgments (`pass`/`fail`) |
| Recall, false-negative rate | conformance judgments, sampled judgments |
| Selector precision | share of selected subjects not judged `notApplicable` |
| Coverage, abstention rate | models that abstain |
| Agreement | the current revision, on the same subjects |

Each decision has an evaluation policy: its precision target and which error it tolerates.

Beyond change classes:
- **Status** follows the ADR lifecycle: `proposed → accepted | rejected`, `accepted → deprecated | superseded`. Split and merge are supersession, and the old judgments are reused only as evidence.
- Moving a decision between a project and a pack is packaging, not a change to the decision. Judgments never travel with it, and models are retrained locally.
- **Promotion** (a prompted model, then a trained one, then a deterministic check) is not a state. It is a sequence of `minor` revisions, each passing evaluation against the current one.

## Vocabulary

Names follow an existing standard wherever one fits, with the standard's meaning:

| Concept | Name | Standard |
| --- | --- | --- |
| Resource envelope | `apiVersion`, `kind`, `metadata`, `spec` | Kubernetes Resource Model |
| Decision record and status | `title`, `status` (`proposed`/`accepted`/`rejected`/`deprecated`/`superseded`), `supersedes`, `consequences` | ADR (Nygard), MADR |
| Requirement wording | MUST, SHOULD, MAY | RFC 2119 |
| Severity | `error`, `warn`, `info` | ESLint levels; mapped to SARIF `error`/`warning`/`note` and LSP Error/Warning/Information |
| Results | `pass`, `fail`, `notApplicable`, `review` | SARIF `result.kind` |
| Suppression | `inSource`, `external`, `justification` | SARIF suppression |
| Finding identity | `partialFingerprints` | SARIF 2.1.0 |
| Provenance | `wasAttributedTo` (a `Person` or `SoftwareAgent`), `wasDerivedFrom`, `generatedAtTime` | W3C PROV-O |
| Classifier outcome | decide or abstain | selective classification (reject option) |
| Model tasks | `classification`, `embedding` | common ML task names (one call shape each) |
| Execution errors | tool execution notifications | SARIF `invocation` |
| Revision change class | `major`, `minor`, `patch` | inspired by Semantic Versioning |
| Evaluation | precision, recall, false-positive rate, coverage, abstention | classification metrics |
| Editor and provider protocol | `initialize`, `textDocument/*`, `window/logMessage`, capabilities in the manifest | Language Server Protocol |
| Options | JSON Schema 2020-12 | JSON Schema |

Lighthouse-specific terms remain only where no standard fits: decision, check, evidence,
judgment, meaning version and check revision.

## Open questions

- **Incident linking.** How a bug fix is recognised as design-relevant without an agent: commit message, linked issue, or test added with the fix?
- **Split detection.** What disagreement between clusters inside one decision justifies a split proposal?
- **Deprecation.** Is silence evidence that a decision is obsolete, or that it works?
- **Cross-project judgments.** Should any travel with a published decision? Default: no.
- **Session domain.** Decisions about agent actions reuse this model, but their subjects are tool calls. Their labels and latency budget are still open.

## Status

| Part | State |
| --- | --- |
| Decisions, packs, verdicts, annotations, `decisions.jsonl`, SQLite memory | done (verdicts in today's form) |
| `check` providers (`builtin`, `cel`, `command`, `model` served by agent review tasks; `rpc` reserved), meaning version and check revision | done |
| Verdicts become judgments and suppressions; naming alignment (`intent` → `context`, provenance fields) | after 2d-2 |
| Judgments on subjects, sampling, evaluation, `Revision` records, log compaction, snapshot budgets | Phase 3 |
| Evidence sources (grouping, mining) and revision proposals | Phase 4 (mining at `init` in Phase 5) |
| `Model` kind: `embedding` (Phase 3), `classification` with layered bindings (Phase 7) | Phase 3, 7 |
