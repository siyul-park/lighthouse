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
| Decision memory | SQLite history, committed `decisions.jsonl`, `lighthouse-disable-next-line` directives, expiring judgments, evidence snapshots, `--format json` |
| Agent loop | MCP server, Claude Code hooks, generated skill, `init --agent`; compact grouped agent output that says how each decision is fixed |
| Autofix | one canonical fix per decision (`ops`, `command`), in-memory verification, rollback, atomic writes, user-level trust |
| Resource model | `apiVersion`/`kind`/`metadata`/`spec` for every spec, JSON Schema per kind, `spec validate`, SARIF, `pattern` renamed to `decision`, severity `error`/`warn`/`info` |
| Check providers | `check:` as `builtin` (standard ops `order`, `cycle`), `cel` with a standard library, `command`, `rpc` (reserved) or `model` (served by agent review tasks); every bundled decision re-expressed with identical findings; `severity` replaces `enforcement`; meaning version separate from check revision; ADR `status`, `supersedes`, `consequences` |
| Speed | per-run fact memo, parallel rule and provider runs (rayon), concurrent Go modules, `--timings` |
| Fewer concepts | `spec::project` replaces config (projects replace presets and overrides); fields cut to requirement/scope/severity/options + context/consequences/status/supersedes + check/fix/examples/provenance; generated and test code handled by each decision's scope; camelCase options; UUID identity; decision ids in the ESLint convention, checked by `core/decision-naming`; ESLint-style `lighthouse-disable` directives; judgments (`pass`/`fail`/`notApplicable`) and SARIF suppressions replace verdicts; `decisions.jsonl` is the only source of truth |
| Self-check and placement | `layers` (import-linter contracts), `feature-envy` and `misplaced-symbol` (Lanza & Marinescu thresholds), `no-hidden-target`; the low-precision heuristics `private-helper-callers`, `unique-type-names`, `tiny-modules` and `owner-file` are the candidate selectors of model checks; enum variants as a symbol kind; option shapes validated |
| Rules aligned with established tools | independent standard limits (cyclomatic, cognitive, statements, depth, params, results, function length) with one limit shape per role and `-1` for none; params and results count the fields of single-use structs; model checks made deterministic where a linter proves it (context-first, stored context, error identity, panics, private types in APIs); `command` checks read SARIF 2.1.0 (`output: sarif`, `select`, `columns`) so common linters are [wrapped](wrapping-linters.md); the [baseline](baseline.md): precision per decision, timings and findings on the reference repositories |
| Info signals | every deterministic info signal below 20% precision was refined at its false-positive cause or retired to a model check's candidate selector |
| One cohesive catalog | similar decisions merged: 117 decisions became 27, variants are options or kinds and every finding carries its kind so precision stays measurable; Clean Code's *Smells and Heuristics* and chapter rules are covered by a decision, recorded as not applicable (14) or pending a named fact (40) |
| Model checks | classification runtime with binding layers (a revision's model, the decision's, the project's, then a review task), abstention falling through, answers cached and budgeted; `decision eval` with precision, recall, coverage, Brier score, calibration and AUROC over the labels, a refit for labels the model learned from, and a promotion gate; `model train`, `attach`, `detach` and `rebuild` for a local classifier trained on judgments (it ties the per-decision constant today, so attach is refused where it does not beat the check) |
| One CLI and MCP surface | noun-verb commands, one meaning per flag, each MCP tool the twin of a command with annotations, an `outputSchema` and structured content; every JSON result has a schema derived from its type (`schema/`); `check --fix` always prints a `CheckReport` and the `fix` tool prints the `FixReport`; the labels of a decision are `labels/<pack>/<decision>.tsv` (`--labels`) |
| Crate boundaries | 15 crates in layers, each one role, the edges enforced by `design/layers`; one name per concept across crates, protocol, schemas and docs (`Run`, `fingerprints`, `decision`) |
| Hooks report the session's own changes | the first edit of a file records its findings; later hooks report only the findings the session introduced, and a turn with no edit is silent |
| Stability signals | module stability, reach of functions and a stability probe on the reference repositories |
| One concept model | a check abstains (the CEL field `abstain`); a provider names the `protocol` it speaks and its reach (`url`, `command` or `plugin`); chat classification always reads logprobs; the core is free of the code model, with `design/layers` keeping `lighthouse_model::ucm` out of core, store, ml, report and session; plugin protocol 0.3: `lighthouse/index`, `initialize` in MCP and LSP vocabulary (`workspaceFolders`, `serverInfo`, `capabilities`), LSP error codes, one method table with its HTTP twins, conformance transcripts replayed by every plugin and the Go sdk |

## Next

Rule accuracy and checking cost are measured; the [baseline](baseline.md) is the yardstick.
What is checked by hand is automated first where the result can be verified, then models
where rules fall short, then reach. Each step below comes before the ones that build on it.

### Fixes by models, verified
The biggest direct cut in agent work, and it needs no labels: the verifier already exists.
- **A `type: model` fix** for the decisions `[fix] decisions` lists, once `[models] agent`
  is bound: an agent edit of a scratch workspace (`agent`: a headless agent CLI, later an ACP
  agent), within the scope `symbol` or `files`. Without an agent the finding stays a review task
  carrying the brief.
  - Headless agent CLIs are `agent` providers (a `command` without a protocol); chat models serve
    classification only.
  - The fixer's output is always verified (apply, format, re-check, roll back) and is always `unsafe`.
  - `[fix] unsafe` (false by default) says whether a verified change is applied without a person; `[rules."<id>"] safety` says it per decision.

### Local models
Free, local and calibrated, run in process; evaluation is the promotion gate.
- A Core ML embedding model on the Apple Neural Engine through the standard Apple path;
  adopted only if it is at least twice as fast with no drop in evaluation.
- A 1B-4B local model as a classifier, measured first on the hand-labelled subjects
  against the per-decision constant, zero- and few-shot; implemented only if an arm
  earns it. No arm tried so far (hosted logprobs models, free chat models, the local
  learner) beats that constant.
- Remote decision models stay behind the published APIs: System One models answer yes/no
  questions with a probability through TypeSafe's protocol, and `:free` chat models label
  candidates on request.

### Checks that author themselves
A check author turns `model` checks and noisy selectors into deterministic revisions that
pass the evaluation gate, which removes review tasks at the source. A revision still needs
approval, and a major change always needs a person.

### Learning from history
Labels are the measured bottleneck of every classifier, so they come next.
- **Repository mining:** change direction, prevalence and kept fixes become weak labels
  (strength `derived`) and `proposed` decisions. Changes are grouped by structural change
  first, then by embeddings; generation drafts the wording. A design comes first.
- **Stability:** a system table and a risk ordering of the signals. Risk orders and
  samples findings; it never removes one.

### More Clean Code coverage
General provider facts for the 40 pending items: flow attributes, event kinds, external
calls, parameter attributes, type origin; effects, locals and clone fingerprints follow.
This adds findings, so it waits until automation exists to absorb them.
- A new deterministic kind ships only if its sampled precision passes; otherwise it is
  judged.
- A project can replace a bundled decision's check (`rules."<id>".check`) to wrap a
  linter it already runs.

### Fast on large repositories
- **Done:** per-unit provider caches with early cutoff and a host result cache keyed by
  each rule's declared reach. Warm runs are byte-identical to cold runs
  (`scripts/fastgate.sh`); a warm run on this repository takes 0.7 s.
- **Next:**
  - type-check edited packages from source over dependency export data;
  - cache failed units;
  - let the host keep fragments, so providers answer "unchanged" (the index half of
    protocol 0.2).
- **Target:** a one-file edit costs under 40% of a cold run, reported per repository.
- **Protocol 0.2, the rest:** LSP lifecycle, text sync, UTF-8 positions, and
  `lighthouse/check` for `rpc` checks.

### The decision loop, completed
- **Graph queries:** decision, finding, judgment (reason, actor), evidence and revision.
- **Proposals:** narrow, widen, demote or promote a decision, with generated examples
  and drafted wording. Nothing is enabled automatically.
- `lighthouse log compact`.

### Decision search
- One `search` tool over decisions, judgments, findings, examples, symbols and
  revisions, for decisions that rules cannot capture precisely.
- **Ranking:** a structured filter, then BM25 (FTS5), the project's embedding model and
  structural signals, fused with Reciprocal Rank Fusion.
- **Hooks** search when a task starts and before a file's first edit, first in shadow
  mode. Agents' answers become judgments.
- **Evaluation:**
  - History labels are weak: a missing record is unlabelled, never irrelevant.
  - The final measure is a TREC-pooled, hand-verified subset.
  - Splits are by repository and commit time; tuning data is kept apart.
  - Results are reported per check type, decision and language.
  - FTS5 BM25 alone is the baseline; embeddings are adopted only if they improve it.

### Packaging and adoption
Public adoption waits until the loop has been shown working.
- Prebuilt binaries that bundle the Go and Rust providers and the local model plugin
  (model weights download on first use), plus a one-command install. Verified on clean
  machines. Not yet a public release.
- `lighthouse init` detects languages and proposes a starter set, including mined
  decisions.
- Public release, and a docs pass: each document holds only what its reader needs.

### Later
| Step | Scope |
| --- | --- |
| Artifact graph | domain-neutral nodes and edges; a markdown provider as the first non-code domain, then the shared artifact and relation graph extracted from what the second domain shows is common |
| Breadth | `lsp-bridge` (any off-the-shelf language server, at lower capability), then native TypeScript and Python providers |
| Ecosystem | `lighthouse lsp` for editors (diagnostics, fixes, judgments); external rule plugins over RPC that ship their decision specs; `plugin add` with a lockfile |
| Session domain | decisions about agent actions; a PreToolUse gate that allows, asks or denies |
| More API specs | Anthropic Messages API; session-domain models |
| Remote models | training and classification on a server declared by base URL, through the published model APIs only: TypeSafe System One for classification, OpenAI Embeddings, Models, Files and Fine-tuning for the rest; a reference server serving both; one conformance suite for in-process, stdio and HTTP; local stays the default |

## Known gaps
- A per-language check, `spec.languages.<id>.check`, does not exist. A decision whose
  deterministic part rests on facts only some providers give (`design/concurrency`,
  `stored-context`, `compare` and `assert` of `design/errors`, `unexported` of `design/signature`: Go only) guards its `where` on
  the language and cannot keep a `model` check for the others.
- `testing/cases` cannot tell a helper that returns or asserts on the target's
  result from one that only builds with it (a data-flow fact: built or consumed against
  returned or asserted), so it is info.
- `design/layers` does not report an `ignore` entry that matches no import (import-linter's
  `unmatched_ignore_imports_alerting`): a cel rule judges one edge at a time.
- Rename fixes are off: no provider declares complete reference sites yet.
- External binaries in `command` providers are trusted by their command line, not by
  content.
- Shallow helpers are not extended: the deterministic check reached 12% precision on two
  samples and no model arm beats the per-decision constant, so the opt-in `helpers`
  review route stays as it is.
