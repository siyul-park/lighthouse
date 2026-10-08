# Rule pipeline

Lighthouse is not a fixed rule pipeline. It is a decision evolution system: a **decision** is
the source of truth, and Lighthouse repeats **evidence → check → judgment → revision** on it.
This page defines that model with as few concepts as possible. The methods behind learned
checks and repository mining are in [learning.md](learning.md). The [roadmap](roadmap.md)
says what exists today (see [Status](#status)).

## Concepts

```text
 Decision ─┬─ meaning       what it demands             → meaning version
           └─ check         how it is evaluated now     → check revision
 Evidence                   what was observed
 Judgment                   pass | fail | notApplicable on one subject
 Suppression                why a fail is not acted on
 Revision                   an approved change to a decision, with its evaluation
```

| Concept | Is | Absorbs |
| --- | --- | --- |
| Decision | what was decided and why, and how it is evaluated (`Decision` resource) | rule, pattern, proposal (a decision with `status: proposed`) |
| Evidence | what was observed, with provenance; carries no result | signal, snapshot, incident, mined edit |
| Judgment | a recorded result on one subject for one meaning version, drawn from evidence | verdict, label, example (an authored judgment on an example subject), audit and exploration samples |
| Suppression | a recorded reason not to act on a `fail` (SARIF suppression) | annotation (`inSource`), intentional exception and won't-fix (`external`) |
| Revision | a change to a decision, with its evaluation and approval | promotion, demotion, narrowing, widening, model attachment |

**Meaning version and check revision are the central axis.**
- A judgment belongs to the meaning version it was made under. It holds while that version and its evidence are unchanged.
- A new check revision (a better predicate, a retrained model, a narrower selector) never expires judgments, so a check improves without losing its history.
- A new meaning version never silently inherits judgments.
  - Older judgments may be reused only as evidence.
  - They become judgments for the new version only after revalidation: by a judge, a person or an agent, or in bulk as one explicit, recorded approval that names the actor.

## Decision

A decision is a `Decision` resource committed with the catalog (see
[architecture](architecture.md)):
- **Meaning:** `requirement` (RFC 2119 wording), `scope`, `severity`, `options`.
- **Record:** `title`, `intent`, `rationale`, `consequences`, `provenance`.
- **Status**, as in MADR: `proposed`, `accepted`, `rejected`, `deprecated`, `superseded`.
- **`supersedes`**, a list. One decision superseding several is a merge; several superseding one is a split.
- **`check`**, optional. A decision without one is documentation: it appears in docs and agent guidance and produces no results.

Severity belongs to the meaning. An authored `error` is *definitive*: it needs no judgment,
and only a suppression waives it.

## Check

A check answers one question: *how is this decision evaluated on a subject now?* It is one
provider, configured like every other provider in Lighthouse (fix, embedder, formatter).
Three layers keep the vocabulary small and standard:

| Layer | Says | Values |
| --- | --- | --- |
| Interface (plugin kind) | what is asked | `Check`; it may delegate to the `Judge` and `Classifier` kinds |
| Runtime (`type`) | where the code runs | `builtin` (in process), `cel` (expression), `command` (process), `rpc` (plugin process) |
| Capabilities (manifest) | how it behaves | `deterministic`, `abstains`, `cost` |

"Deterministic", "learned" and "judge" are therefore not types. They are capability
profiles of providers:

| Profile | Capabilities | Examples | On failure |
| --- | --- | --- | --- |
| deterministic | `deterministic: true`, never abstains, cheap | builtin `order`/`proximity`/`cycle`, `cel`, `command`, `rpc` checks | analysis incomplete (exit 3) |
| learned | `deterministic: false`, `abstains: true`, cheap | a `Classifier` provider attached by a revision | skipped with a notice |
| judge | `deterministic: false`, expensive | builtin op `judge`, delegating to the project's bound `Judge` provider, else an agent review task | skipped with a notice |

The engine keys behaviour (incomplete or skipped, definitive or reviewable, routing) off the
capabilities, never off type or op names. A new provider fits by declaring its capabilities.

```yaml
check:
  type: builtin
  op: judge
  select: 'node.kind == "function" && node.changed'
  prompt: Does the name state the result rather than the mechanism?
  shots: examples   # the decision's examples are the few-shot set
```

Which `Judge` serves `op: judge` is a project binding, like a provider configuration in
other tools. A decision says what to ask; the project chooses who answers.

A judge check can be **accelerated by a learned model**. The model is not authored: an
approved revision attaches it, trained on the decision's judgments (see
[learning.md](learning.md)). Routing follows capabilities, cheapest confident provider first:

```text
 candidate ─► learned (abstains): P(fail) ≥ upper → fail · ≤ lower → pass · else ─► judge
```

- The order is about **cost and confidence**, not quality. A learned model approximates a boundary that has no faithful predicate; it is not a weaker deterministic rule.
- A deterministic check decides every subject it selects. Partial deterministic coverage of a judged decision is expressed by narrowing the judge's `select`, not by stacking providers.
- Deterministic evaluation is the **preferred** endpoint when the boundary can be expressed faithfully. It is not a required one: some decisions stay judged for good.

## Evidence

Evidence is what was observed. It carries no result.
- **Origin:** what brought it up.
- **Provenance:** who recorded it and what it came from, in W3C PROV-O terms.

A **producer** draws judgments from evidence: a person, an agent, a judge, a learned model,
or a mining rule.

| Origin | Examples | Judgments a producer may draw |
| --- | --- | --- |
| Intent | a person or agent decides up front | example judgments |
| Existing artifact | a convention, an ADR, a style guide, an external standard; an agent writes the decision (no importer) | example judgments |
| Existing tool rule | a golangci-lint, clippy, ruff or semgrep rule, or a script, wrapped by `command`/`rpc` | the tool's `fail` results, then judgments on them |
| Recorded history | judgments, suppressions, repeated fixes of one shape, review comments, agent-session corrections | judgments with the same polarity |
| Incident | a bug fix, postmortem, CI break or revert | the incident site `fail`, the fixed code `pass` |
| Repository mining | change direction in refactor-like commits; prevalence in the tree ([learning.md](learning.md)) | weak `fail` (before, outlier) and `pass` (after, dominant) |
| Snapshot | current metrics frozen as a budget | the baseline; the decision is a ratchet |
| Another project | an organisation catalog, a third-party pack | its examples (its other judgments never travel) |

- **Decision author:** a person, an agent, or Lighthouse. Lighthouse only *proposes* (`status: proposed`); a person or agent accepts.
- **Judgment strength** follows the producer: human > agent > incident > judge > learned model > mining rule. A stronger judgment on the same subject and meaning version supersedes a weaker one. A check whose judgments are mostly weak is capped at `warn`/`info`.

Mining, grouping and similarity are evidence producers. The model holds without them:
intent → decision is a complete loop.

## Judgment and suppression

A **judgment** is a result on one subject: `pass`, `fail` or `notApplicable` (the decision
does not apply to this subject). It is a recorded label, not ground truth.

What used to be a verdict on a finding is now one judgment, plus a suppression when the
finding is right but will not be acted on:

| Reviewer says | Judgment | Suppression |
| --- | --- | --- |
| the finding is right (`fixed`, `accepted-debt`) | `fail` | — |
| false positive | `pass` | — |
| the decision should not apply here (scope too broad) | `notApplicable` | — |
| intentional exception, project allows it | `fail` | `external`, with a justification |
| won't fix | `fail` | `external`, justification `wont-fix` |
| `lighthouse:allow <decision> -- <reason>` in code | — | `inSource`, with the reason |
| not sure yet | — (stays `review`) | — |

What a run **reports** uses SARIF results with their standard meaning:
- `fail` is a finding. A suppressed `fail` stays in SARIF with its suppression.
- `review` is a case nobody has decided yet.
- `notApplicable` is a subject outside the selector, or judged so.

Judgments also exist where no finding does:
- **audit samples**, which a deterministic or learned evaluator left unflagged;
- **exploration samples**, a fixed fraction of confident learned calls re-judged;
- judge results on candidates.

These give recall and drift, not only precision.

Judgments and suppressions are appended to `.lighthouse/decisions.jsonl` (shared through git)
and cached in SQLite. `lighthouse log compact` folds expired and superseded records; the raw
history stays in git.

## Revision

A revision is a change to a decision: `{decision, parent, meaningVersion, checkRevision,
change, evaluation, approval}`.
- Authored edits to the YAML are revisions whose history is git.
- Derived parts (a trained model, thresholds) and approvals are `Revision` records in `decisions.jsonl`.
- Lighthouse proposes revisions; it never applies one without approval.

**`change` is derived, never declared.** The classification is inspired by Semantic
Versioning but is not SemVer: it says which version of the decision moved.

| `change` | When | Examples | Evaluation | Judgments |
| --- | --- | --- | --- | --- |
| `major` | the meaning version changed | a stricter requirement, a new option default | required | reused only as evidence, revalidated |
| `minor` | only the check revision changed | replace `op: judge` with a `cel` check, attach or retrain a learned model, narrow or widen a selector | required, against the current revision | kept |
| `patch` | neither changed | wording, rationale | none | kept |

**Evaluation** replays the candidate against the decision's judgments, examples included:

| Metric | Measured on |
| --- | --- |
| Precision, false-positive rate | `fail` judgments |
| Recall, false-negative rate | `pass`/`fail` judgments, audit samples |
| Coverage, abstention rate | the learned model |
| Agreement | the current revision, on the same subjects |

Each decision has an evaluation policy: its precision target and which error it tolerates.

Beyond change classes:
- **Status** follows the ADR lifecycle: `proposed → accepted | rejected`, `accepted → deprecated | superseded`. Split and merge are supersession, and the old judgments are reused only as evidence.
- **Distribution** uses package-manager terms:
  - `publish` moves a decision into an organisation pack;
  - `add` enables a pack in a project.

  Judgments never travel. Models are retrained and thresholds recalibrated locally.
- **Promotion** (judge → learned → deterministic) is not a state. It is a sequence of `minor` revisions, each passing evaluation against the current one.

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
| Learned outcome | decide or abstain | selective classification (reject option) |
| Revision change class | `major`, `minor`, `patch` | inspired by Semantic Versioning |
| Distribution | `publish`, `add` | package managers (cargo, npm) |
| Evaluation | precision, recall, false-positive rate, coverage, abstention | classification metrics |
| Editor and provider protocol | `initialize`, `textDocument/*`, `window/logMessage`, capabilities in the manifest | Language Server Protocol |
| Options | JSON Schema 2020-12 | JSON Schema |

Lighthouse-specific terms remain only where no standard fits: decision, check, judge,
evidence, meaning version and check revision.

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
| `check` providers by runtime, builtin op `judge` (served by agent review tasks), provider capabilities, meaning version and check revision | in progress (2d-2) |
| Verdicts become judgments and suppressions; naming alignment (`intent` → `context`, provenance fields) | after 2d-2 |
| Judgments on subjects, audit sampling, evaluation, `Revision` records, log compaction, snapshot budgets | Phase 3 |
| Evidence producers (grouping, mining) and revision proposals | Phase 4 (mining at `init` in Phase 5) |
| Judge providers, learned models, Embedder | Phase 7 (Embedder starts in Phase 3) |
