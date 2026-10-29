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
| Judgment | a recorded result on one subject for one meaning version, drawn from evidence: conformance (`pass`/`fail`) or applicability (`notApplicable`) | label, example (an authored judgment on an example subject), audit and exploration samples |
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
verify):
- **`type`** names where evaluation happens: `cel` (an expression), `command` (a process), `rpc` (a plugin process) or `model` (the bound classification model).
- **Capabilities** in the manifest say how it behaves: `deterministic`, `abstains`, `cost`.

**Fix.** A decision's `fix` is `cel`, `command`, `rpc` or `model`. A command is a [program](programs.md), the same shape as a command check. A `command` check or fix that differs by language leaves its `command` out and gives one program per language under `languages.<id>.check` and `languages.<id>.fix` (`core/formatted`: gofmt and rustfmt); a file of a language with no program is not checked or fixed by it. A model fix is made by an agent that edits a scratch copy of the project (`[models] agent`) for the decisions `[fix] decisions` lists, within the scope `symbol` or `files`; with no agent bound the finding stays a review task carrying the brief. An agent's work is a guess, so it is kept only when the scope holds, the finding is gone, nothing of `warn` or `error` is new in the changed and referencing files and they are fully analyzed; `[fix] verify` programs must also pass over the fixed texts. A kept fix is written when `[fix] unsafe = true` or the run has `--unsafe-fixes`, and otherwise reported as a preview. Details: [models.md](models.md#fixes-by-a-model), [fixer-spec.md](fixer-spec.md).

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
- **Candidates:** the subjects in the decision's `scope`. An optional `select` (CEL) narrows them. Only subjects of a file (a symbol, function, file, test, comment or event) can be judged: a model needs their source.
- **Output:** fixed by the task, so there is nothing to configure: the probability that the subject fails (TypeSafe System One's `noul`). The host maps it to `fail` or `review` (the SARIF result kinds) with one band per (model, decision): at or above `upper` it is a `fail`, at or below `lower` a `pass`, otherwise it is left to the next binding. Until `eval` has calibrated a band, `upper` is 0.9 and nothing is cleared. The cache keeps the raw probability, so a new band never invalidates it. There is no reason.

`type: model` is the same runtime as a `cel` check whose `where` is `false` and whose `abstain` is the `select`. A `cel` check that can prove part of a decision says so in the same place:

```yaml
check:
  type: cel
  where: func.envy.count >= 3 && size(func.envy.others) == 0   # proven: a finding
  abstain: func.envy.count >= 2                                 # suspected: ask the bound model
  message: "{{ func.name }} depends on {{ func.envy.target_name }}'s members only"
```

`where` true is a `fail`; `where` false and `abstain` true is an **abstention**; anything else is a `pass`. Abstained subjects fall through the bindings below like a model's own abstention. A `fail` from a model is a finding with the check's own message, evidence, identity and fix, plus `evidence.confidence` and a `modelVersion` (`<provider>/<model>@<version>`) beside the evidence: the version of the program moves with its binary and environment, so it is recorded with the finding and the judgment and never compared to expire one. Its level is at most `warn`.

Models are one plugin kind, `Model`, with four tasks (`tasks` in the manifest), because each
task has its own call shape and standard body:

| Task | Call | Body | Built |
| --- | --- | --- | --- |
| `classification` | subject + decision (requirement or prompt, examples) → P(fail), or abstain | TypeSafe System One | yes |
| `embedding` | texts → vectors | OpenAI Embeddings | later; bound per project only |
| `agent` | an agent session in a scratch copy; changed files go through the verifier | Agent Client Protocol, or a headless CLI | with fixes by models |

`agent` is generative only. `classification` is not: its probability comes from the
model's own distribution (a classifier's score, a System One `noul`, the logprobs of a chat
model's answer token), so a verbalized number and an agent CLI are never classifiers.

How a model arrives at its distribution is its own business. A chat model with logprobs (read through Chat Completions, never to generate), a
jev-class System 1 model, or a classifier trained on the decision's judgments all serve
`classification` the same way.

**A decision never names a model.** Decisions are shared through git and packs, while which
models exist (local or remote, allowed or not, at what cost) depends on the machine and the
project. A decision says what to ask; bindings say who answers. A **binding layer** is chosen per
decision, most specific first, and an **abstention falls through** to the next one (layers exist
only for `classification`; the other tasks bind once per project):

| Layer | Where | Set by |
| --- | --- | --- |
| 1. Trained for this decision | a `Revision` record in `decisions.jsonl` naming the model id and version; the artifact is local and rebuildable | Lighthouse trains on request (`lighthouse model train <decision>`); a person or agent approves the revision |
| 2. Project, per decision | `[rules."<id>"] model = "…"` (one model or a route) | the project |
| 3. Project default | `[models] classification = "…"` (one model or a route), `embedding = "…"` | the project |
| 4. Agent | a review task | always available |

A model is named `<provider>/<model>`, split at the first `/`. A provider has two axes: the **protocol** it speaks (`protocol`: `openai`, `typesafe`, or a list in order of preference) and the **reach** to it, exactly one of `url` (HTTP, served by `model-http`), `command` (a stdio process) and `plugin` (an rpc plugin). The standard protocols are built in at their own service (`openai`: api.openai.com/v1, key in `OPENAI_API_KEY`; `typesafe`: api.typesafe.ai, key in `TYPESAFE_API_KEY`), so `protocol = "openai"` alone is a provider; any other service is one the project declares. OpenRouter, Ollama, vLLM and LM Studio speak the `openai` protocol:

```toml
[models]
classification = "openrouter/qwen/qwen3-coder:free"  # or a route: ["nvidia/nvidia/nemotron-3-super-120b-a12b", "openrouter/nvidia/nemotron-3-super-120b-a12b:free"]
routing = "ordered"               # or "round-robin" for a route of several
budget = 200                      # subjects per run put to models that cost; the rest stay `review`
explore = 0.05                   # share of an attached model's fails sent on to the next layer, to keep labels coming; 0 to 1

[models.providers.openrouter]     # a recipe: any OpenAI-compatible service; free models first
protocol = "openai"               # or a list: ["openai", "typesafe"]
url = "https://openrouter.ai/api/v1"
headers = { Authorization = "Bearer ${OPENROUTER_API_KEY}" }   # the key stays in the environment
cost = 1                          # counted against the budget = the daily free quota
# a model whose reasoning is forced needs room for it, and the answer is read past the reasoning tag
body = { provider = { require_parameters = true }, reasoning = { effort = "low" }, max_tokens = 1024 }

[models.providers.mercury]        # System One on OpenRouter: P(fail) from the model itself
protocol = "typesafe"
url = "https://openrouter.ai/api"
headers = { Authorization = "Bearer ${OPENROUTER_API_KEY}" }
cost = 1
# reference: mercury/inception/mercury-decide:free

[models.providers.ollama]         # a local one needs no key
protocol = "openai"
url = "http://localhost:11434/v1"
cost = 0

[models.providers.lmstudio]       # LM Studio; vLLM is the same at http://localhost:8000/v1
protocol = "openai"
url = "http://localhost:1234/v1"
cost = 0

[models.providers.nvidia]         # NVIDIA NIM: free, rate-limited; logprobs are rounded to 0.5 nats
protocol = "openai"
url = "https://integrate.api.nvidia.com/v1"
headers = { Authorization = "Bearer ${NVIDIA_API_KEY}" }
body = { chat_template_kwargs = { enable_thinking = false } }  # the first token is the answer
cost = 1
# reference: nvidia/nvidia/nemotron-3-super-120b-a12b, nvidia/nvidia/nemotron-3.5-lightning-30b-a3b

[models.providers.script]         # any program that reads System One's body and prints its response
protocol = "typesafe"
command = ["./scripts/classify", "{model}"]
# pointer = "/result"             # RFC 6901 pointer to the response inside an envelope
timeout = "120s"
# reference: script/fast

[models.providers.work]           # a plugin that offers models; its handshake says what it serves
plugin = "model-local"
# `local` is built in and needs no declaration: model = "local/learned@sha256:<hex>"

[rules."design/no-redundant-qualifier-result"]
level = "info"
model = "ollama/qwen3-coder:30b"  # a cheaper model for this decision only
evaluation = { precision = 0.9 }          # recall = 0.8 optional
```

- **How a chat model answers.** From its own distribution, never from a number it writes: the model is asked for one word, `true` if the subject violates the requirement and `false` if it complies (the names of the criteria in the prompt), with `logprobs`, `top_logprobs = 20`, `max_tokens = 1` and `temperature = 0`; P(fail) is p_true / (p_true + p_false) over the first answer token's top log probabilities, after trimming, lower-casing and stripping punctuation (the OpenAI Cookbook's classification recipe, renormalized over the labels as in Zhao et al., ICML 2021). The answer token is the first sampled token that is neither blank nor a closing reasoning tag such as `</think>`. Neither label among the top 20 is an abstention; a response without `logprobs` is a provider error ("the model returned no logprobs; it cannot serve classification"), so the route moves to its next model or the subject stays for review. Use a non-reasoning model, or one whose reasoning is merely forced: a model that reasons needs room for it, as in the `openrouter` recipe.
- **`body`** is a JSON object merged shallowly into every request body of an HTTP provider (the OpenAI SDK's `extra_body`); its keys override the body's own, `max_tokens` included. It carries service-specific parameters, such as OpenRouter's `provider.require_parameters`, without code for any service.
- **A route.** Layers and routes are different things: a layer moves on when its models *abstain*, a route moves on when a model *fails*. A binding is one reference or a list of equivalent ones; a list is one layer, and a layer is asked as one: its models are tried in turn.
  - `routing = "ordered"` (default) starts every request at the first model; `"round-robin"` starts consecutive requests at consecutive models, the others following in the order written, wrapping.
  - A request moves to the next model only when a model fails (an HTTP error including 429 and 5xx, a timeout, missing `logprobs`, unusable output) or the budget refuses a model that costs. An abstention or an unsure answer goes to the next layer, never the next model.
  - A model that fails 3 times in a run is skipped for the rest of it, with one notice (LiteLLM's `allowed_fails`). With every model out, the subject stays for review.
  - A kept answer of any model of the route is used before a request is sent; the model that answered is the one `modelVersion` and `eval`'s `scores[].model` name, and `eval` lists the run's notices per check, so a failing model is not silent.
- A `url` provider, and a bare `protocol`, is served by the `model-http` plugin, which must be listed in `plugins`; its `timeout` (default 120s) is the limit of one HTTP request, and a `plugin` provider has none. A `command` provider speaks `typesafe`: System One's body on stdin, the response on stdout, picked out of an envelope by `pointer` when it has one. It is a [program](programs.md) (no shell, its own process group, a time limit, a cap on output, the one environment plus the declared `env`, project trust) and its program is part of what trust covers. An answer that does not fit the shape is no answer, and the subject stays for review. A `command` without a protocol speaks plain text, which has no probability distribution: stdin is the brief, stdout the agent's notes, and it serves `agent` only, run in a scratch copy of the project; `protocol = "openai"` on a `command` is rejected. Binding a provider to a task it does not serve is a configuration error. Fixes by models are in [models.md](models.md#fixes-by-a-model) and, by the agent that edits, [models.md](models.md#the-agent-that-edits).
- The engine keys off the capabilities a model declares, never its name: `deterministic`, `abstains` and `cost`. The budget and the report scope (changed files, or everything with `--all-models`) limit only the models with a `cost` above 0; an answer is cached under the model, its version, the check's revision and the subject. A model that fails never makes a run incomplete (exit 3): its subjects stay for review.
- Subjects nothing decided (an abstention, an unsure answer, no bound model, the budget spent, a failed model, `--no-models`) are reported as SARIF results of kind `review`, in the report scope only, and as review tasks in the agent output, grouped. They never change the exit code.
- `--record-model-labels` records what models fail as `Judgment`s attributed to the model (`<provider>/<model>@<version>`), so that training can reproduce them from `decisions.jsonl`. A person's judgment still wins.
- The layer-1 reader is in: a `Revision` record (`{decision, parent, meaningVersion, checkRevision, change, evaluation, approval, model}`, `approval` being PROV `wasAttributedTo`) binds its `model` to the decision before every other layer; one without a `model` unbinds it (a revision is a full snapshot, so a writer copies its parent's model). `lighthouse model attach` and `model detach` write it.
- `lighthouse decision eval <decision>` measures the check against the labels a person gave or an example shows (weight 1.0; an agent's or a model's label never grades): precision and FPR over conformance judgments, recall and FNR, the selector precision (the share of selected subjects not judged `notApplicable`, kept apart), coverage, the Brier score, the expected calibration error (ECE, 10 equal-width bins) and the AUROC of the probabilities models gave (over the labelled subjects they answered, counted beside them; abstentions are counted, not scored; the AUROC is null when a class is missing), and with `--check <file>` the agreement of a candidate check with the current one; every rate with a Wilson 95% interval. `--audit N` writes unflagged selected subjects to a table for a person to label, to measure recall. It counts the model calls and their cost, and gives a verdict against the policy by the point estimates, the intervals shown beside. The policy it is judged by (`evaluation`: a precision target, default 0.9, and an optional recall target) is project configuration, not a decision option, because options are meaning and a policy change must not move the meaning version.
- `lighthouse model train <decision>` collects the same labels (only judgments under the current meaning version, one label per subject, the strongest source winning: person and reviewed table 1.0, agent 0.7, model 0.5; never the labels of the model being trained), builds the subjects exactly as `classify` does (an example is not a row: the question shows it), and calls `lighthouse/train` on a model that learns. It writes the dataset to `.lighthouse/datasets/sha256/<hex>.jsonl` (see [models.md](models.md)) and nothing to `decisions.jsonl`.
- `lighthouse decision eval <decision> --model <ref>` grades a model as layer 1 beside the check: the base run leaves layer 1 out, and the model is asked only about the labelled subjects the check leaves for review, a P(fail) at or above `upper` turning them to `fail`. A model fitted on labels is not graded on them as it is: each label it learned from is graded by a refit on the other folds (the folds the trainer cross-validates by), so the answer is one the model never saw.
- `lighthouse model attach <decision> --model <ref>` runs that evaluation and appends a `Revision` only when the candidate meets the policy, loses no recall, newly fails none of the decision's valid examples and decides a labelled subject the check leaves for review; otherwise it exits 1 with the reasons and writes nothing, and nothing forces it. The revision records the numbers and the dataset the model learned from; `approval` is `--attributed-to`: the repository's git user or, through MCP, the agent.
- While a model is attached, `decision eval` grades it as layer 1 and proposes `detach` when it misses the policy. Nothing is written until `lighthouse model detach <decision>` appends a revision without a model; a proposal is never applied by itself.
- **`local`** is the built-in provider of the `model-local` plugin (cost 0, deterministic, never abstains), which must be listed in `plugins`. Its one model, `learned`, is trained from the labels, never prompted: `lighthouse model train <decision> --model local/learned` embeds the labelled subjects with EmbeddingGemma-300M, fits a rate, nearest neighbours (k = 5) and an L2 logistic regression, judges each by its out-of-fold Brier score (folds by file, so a file is never on both sides), keeps the simplest within one standard error of the best, and calibrates it by Platt scaling. Below 10 labels in a class only the rate is trained. The model is the file `.lighthouse/models/sha256/<hex>.json`, named by the digest of its canonical form (RFC 8785); `train` prints its card (rows, candidates with their scores, the chosen one, Brier, AUROC, the band of probabilities that is surely a fail or a pass) and the reference `local/learned@sha256:<hex>`. Bind that reference like any model (`model = "local/learned@sha256:<hex>"`); a plain `local/learned` is an error telling to train first, and a missing or altered file is an error naming its path and the digest expected. Training the same dataset twice gives the same digest, and the answer cache keys on the digest. The first train downloads the embedder's weights (large, to `FASTEMBED_CACHE_DIR`, else `$XDG_CACHE_HOME/lighthouse/fastembed`, else `~/.cache/lighthouse/fastembed`); it is not done below 10 labels in a class, nor to classify with a rate.

- A cheap model that abstains in front of an expensive one is just layer 1 or 2 over layer 3. It is not a separate mechanism.
- If a layer-1 artifact is missing on a machine (a fresh clone, say), the model is skipped with a notice that names `lighthouse model rebuild`, and the case falls through; `check` never trains.
- `lighthouse model rebuild [<decision>...]` makes the missing artifacts of attached models again. It trains the model on the dataset its attaching revision names (`evaluation.trainingFile`), or on the dataset today's labels make when that file is gone and the digest is the same; a trainer that is not deterministic or does not learn is `not-rebuildable`, other labels are `labels-changed` and nothing is trained, a file or a training run with another digest is `differs` (nothing is deleted), and a model without a digest is `not-local`. It exits 1 unless every model is `present`, `rebuilt` or `not-local`.
- **Exploring.** A model that is surest of a fail would otherwise never be checked. A share `[models] explore` (default 0.05, a number from 0 to 1; anything else is a configuration error) of the subjects the attached model fails is treated as an abstention, so the layers below judge them and a person or agent can label them. A subject is in the share when the first 8 hex digits of sha256(`rule`, NUL, subject key), as a number out of 2^32, are below `explore`: the same subjects every run. Only the attached model, layer 1, is explored, and nothing changes where none is attached. A subject nothing below decides stays open with the reason `explored: <model> said fail at <confidence>`; one a lower layer fails carries `explored: {confidence}` in its evidence.
- `embedding` is bound only per project. Classifiers trained on embeddings record the embedding model in their version tuple, and changing the embedding model invalidates them.
- The order is about **cost and confidence**, not quality. A trained classifier approximates a boundary that has no faithful predicate; it is not a weaker deterministic rule.
- A deterministic check decides every subject it selects unless it abstains (`abstain`); abstained subjects fall through to the model bindings, like a model's abstention. Providers are never stacked.
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

A reviewer records one judgment on a finding, plus a suppression when the finding is
right but will not be acted on:

| Reviewer says | Judgment | Suppression |
| --- | --- | --- |
| the finding is right | `fail` | — |
| false positive | `pass` | — |
| the decision should not apply here (scope too broad) | `notApplicable` | — |
| the finding is right and stays (an intentional exception, the project allows it, won't fix) | `fail` | `external`, with a justification |
| `lighthouse-disable-next-line <decision> -- <reason>` (or `-line`, or a `lighthouse-disable` range) in code | — | `inSource`, with the reason |
| not sure yet | nothing is recorded (it stays `review`) | — |

A judgment carries `wasAttributedTo` (a `Person` or a `SoftwareAgent`) and `generatedAtTime`
(W3C PROV), and belongs to the decision's `meaningVersion`: it stops standing when that
moves.

**Which judgment stands.** Several judgments may exist for one subject. Among those given
under the decision's current `meaningVersion` the stronger attribution stands: a `Person`
over a `SoftwareAgent`, and an agent over a model. A weaker judgment is recorded but does
not replace a stronger one, so an agent cannot undo what a person decided. At equal strength
the later `generatedAtTime` stands, then the larger record id. `generatedAtTime` is the
clock of the machine that wrote the record, so two clones whose clocks differ by more than
the gap between their judgments can disagree about which is later; the record id only
makes the choice deterministic, it does not make it right. When no judgment was given under
the current meaning version, the same order picks one and it has expired: the finding is
reported again with a note. A judgment is matched to the findings a run sees by fingerprint,
meaning version and evidence digest, from the decision log alone, so a clone that holds only
the log applies it. The two kinds are the only records of the decision log; they are one line each, a
`Suppression` naming the id of the `Judgment` it goes with.

What a run **reports** uses SARIF results with their standard meaning:
- `fail` is a finding. A suppressed `fail` stays in SARIF with its suppression.
- `review` is a case nobody has decided yet.
- `notApplicable` is a subject outside the selector, or judged so.

Judgments also exist where no finding does. **Sampling** re-judges a fixed fraction of
subjects a check already decided, unflagged ones and confident ones alike, with a stronger
attribution. This gives recall and drift, not only precision.

A finding's identity (its fingerprint) is seeded by the decision's uid, so judgments survive
a rename; see [architecture](architecture.md#fingerprints).

Judgments and suppressions are appended to `.lighthouse/decisions.jsonl` (shared through git)
and cached in SQLite. `lighthouse log compact` (planned, not built yet) is to fold expired and superseded records; the raw
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
| Coverage | models that abstain; what they do not cover is left for review |
| Agreement | the current revision, on the same subjects |

Each decision has an evaluation policy: its precision target and, optionally, a recall target.

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
| Evaluation | precision, recall, false-positive rate, coverage | classification metrics |
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
| Decisions, packs, judgments, suppressions (reviewer-declared and in the code), `decisions.jsonl`, SQLite memory | done |
| `check` providers (`cel` with `abstain`, `command`, `model`), meaning version and check revision | done |
| `classification` with layered bindings (Revision, rule, project, agent), `provider/model` references, `model-http` (`openai` protocol) and `command` providers, answer cache, budget, SARIF `review` results, `decision eval`, the label collector, `model train` plumbing | done |
| Judgments and suppressions replace verdicts; naming alignment (`intent` → `context`, provenance fields) | done |
| Judgments on subjects, sampling, evaluation, `Revision` records, log compaction, snapshot budgets | Phase 3 |
| Evidence sources (grouping, mining) and revision proposals | Phase 4 (mining at `init` in Phase 5) |
| `Model` kind: `embedding`, trained layer-1 models, `model attach` | Phase 3, 7 |
