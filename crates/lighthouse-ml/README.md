# lighthouse-ml

The host side of mlops: the labels of a decision, the dataset they make, the training of a model on it, and the evaluation of a check against the labels.

**Layer.** L3; depends on engine, model, plugin and spec. Never on checks, rpc, store or session.

## Entry points
- `labels::Collect`, `Judged`, `Shown`, `Label`: subjects and their one label each.
- `dataset::Dataset`: the one writer of training files.
- `train::train`, `TrainRequest`, `Trained`: asks a model for a fine-tuning job on a dataset.
- `evaluate::grade`, `repos`, `EvalRequest`, `EvalReport`: a check (and a candidate beside it) against the labels.
- `metrics::Rate`, `evaluate`, `agreement`: proportions with Wilson 95% intervals.

## Invariants
- Session finds the subjects (it runs the checks and holds the registry and store) and hands them over; ml defines its own `Judged` and `Shown` inputs, so store and ml stay peers.
- Only judgments under the decision's current meaning version count. A label attributed to the model being trained or evaluated is left out, so a model never learns from itself. The strongest source wins: person, reviewed table, coding agent, model.
- Datasets are content-addressed in `.lighthouse/datasets/sha256/<hex>.jsonl` with canonical JSON lines, so the same labels make the same file.
- Recall is a bound, not a measurement: labels come mostly from flagged findings.

## Tests
`crates/lighthouse-ml/tests/it`; `cargo test -p lighthouse-ml`. Models and mlops in [docs/models.md](../../docs/models.md) and [docs/learning.md](../../docs/learning.md).
