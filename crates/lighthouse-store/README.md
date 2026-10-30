# lighthouse-store

The SQLite store: findings, judgments, signals and records. Layer L3. Never: engine.

The quality store: findings with their history in a local SQLite cache, and the judgments, suppressions and revisions on them in a committed, append-only decision log.

**Depends on:** `lighthouse-model`, `lighthouse-protocol`.

## Entry points

- `Store`: open, record a run, query findings.
- `FindingRecord`, `Standing`, `StatusFilter`: a finding and whether a judgment stands on it.
- `NewJudgment`, `JudgmentEvent`, `SuppressionEvent`: what is recorded.
- `Records`, `append_revision`: the log (`.lighthouse/decisions.jsonl`).
- `descriptors()`: schemas of the log records.

## Invariants

- The log is the truth and the database (`.lighthouse/lighthouse.db`) is derived from it. One schema, no migrations: a database of another version is emptied and filled again from the log.
- The log is append-only and committed with `merge=union`, so branches merge cleanly.
- A judgment stands only under the meaning version it was given under; errors are never hidden by a judgment.
- Labels for learning: `fail` is positive, `pass` and `notApplicable` are negative, a `fail` kept by a suppression is its own target, an unjudged finding has none.

## Tests

`crates/lighthouse-store/tests/it`; `cargo test -p lighthouse-store`. Memory model in [docs/architecture.md](../../docs/architecture.md#memory-findings-judgments-and-the-feedback-loop).
