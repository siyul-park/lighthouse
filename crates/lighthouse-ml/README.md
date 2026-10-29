# lighthouse-ml

The host side of mlops: the labels of a decision, the dataset they make, the training of a model on it, and the evaluation of a check against the labels.

**Layer.** L3; depends on engine, model, plugin and spec. Never on checks, rpc, store or session.

## Entry points

- `labels::Labels`, `Judged`, `Table`, `Label`: subjects and their one label each.
- `train::Rows`, `Fit`, `train`, `Trained`: the rows of the labelled subjects, their dataset (the one writer of training files) and the fine-tuning job on it.
- `rebuild::Learned`: a model that is a file, checked and made again from its dataset.
- `evaluate::grade`, `RepoResult`, `EvalReport::pool`, `Overlay`: a check (and a candidate or an attached model beside it) against the labels, pooled over repositories.
- `metrics::Metrics::of`, `Rate`: the metrics, with Wilson 95% intervals.

## Invariants

- Session finds the subjects (it runs the checks and holds the registry and store) and hands them over; ml defines its own `Judged` input, so store and ml stay peers.
- Only judgments under the decision's current meaning version count. A label attributed to the model being trained or evaluated is left out, so a model never learns from itself. The strongest source wins: person, reviewed table, coding agent, model.
- Datasets are content-addressed in `.lighthouse/datasets/sha256/<hex>.jsonl` with canonical JSON lines, so the same labels make the same file.
- Recall is a bound, not a measurement: labels come mostly from flagged findings.

## Tests

`crates/lighthouse-ml/tests/it`; `cargo test -p lighthouse-ml`. Models and mlops in [docs/models.md](../../docs/models.md) and [docs/learning.md](../../docs/learning.md).
