# lighthouse-engine

Runs the decisions over an indexed project: reads and indexes the files, runs the rules, applies annotations, caches and batches model questions, and runs the fix loop that re-checks.

**Layer.** L2; depends on model, plugin, process, resource and spec (no protocol, no checks).

## Entry points

- `Engine::new`, `check`, `check_overlaid`: a run over the files, or over files with in-memory texts (`Overlays`); the `Run` carries findings, gaps and facts.
- `Engine::fix`, `preview_fixes`: the fix loop and its dry description; `FixPlan`, `FixRun`, `FixReport`.
- `Run::exit_code`: 0, 1 (findings), 3 (`EXIT_INCOMPLETE`).
- `RuleTester`: runs the examples of decisions through the whole engine.
- `cache::Cache`: the result cache (`.lighthouse/cache/results.db`).
- `CommandModels`, `ModelOptions`: model asking; `Timings`.

## Invariants

- The core knows no language and no rule: it takes the registry and the catalog it runs over.
- Cache: any doubt is a miss. The key holds everything a finding depends on (build, rule and revision, applicability and options, the digests the rule's reach asks for); changing it wrongly serves stale results. The cache is local and never committed.
- Fix: this is the only writer. It refuses to start on an incomplete analysis, verifies every round by a check over in-memory texts, rolls back any file that got worse (a multi-file fix whole), stops after `MAX_ROUNDS`, and writes only if each file still has the hash it was read with.
- Source annotations (`lighthouse-disable...`) are applied after the rules; a directive that matches nothing is itself reported.
- Models are asked in as few batched requests as possible, most specific binding first.

## Tests

`crates/lighthouse-engine/tests/it`; `cargo test -p lighthouse-engine`. Design in [docs/architecture.md](../../docs/architecture.md#fixing).
