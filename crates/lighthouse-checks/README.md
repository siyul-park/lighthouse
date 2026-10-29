# lighthouse-checks

The checks of the bundled decision packs and the machinery that compiles a decision's `check` and `fix` into a `Rule` and a `Fixer`.

**Layer.** L2; depends on model, plugin, process, resource and spec.

## Entry points

- `registry()`: one `Declarative` plugin per pack of the bundled catalog, plus the `metrics` analyzers under `core`.
- `Declarative::from_catalog`: the rules and fixers of one pack (also used for the project's own layer).
- `reach`: how far a rule's facts and functions reach, which keys the result cache.
- `metrics`, `layout`: the per-function analyzers; the declaration order and owner vocabulary the ordering checks share.
- `command`: wrapped linters (SARIF in, findings out).
- `probe`: test hook, hidden from the docs.

## Invariants

- A rule's `Reach` is the greatest of what its expressions mention. A fact or function missing from `reach.rs` fails a test; declaring one too narrow means a stale cached result.
- A `cel` check selects a subject kind (`symbol`, `function`, `edge`, `module`, `file`, `test`) and flags it when `where` is true; severity, scope and evidence come from the decision.
- Check kinds `cel` and `command` run here; `rpc` is reserved and `model` is agent review.
- Plugin and rule ids are the packs' own; `registry()` panics if the bundled analyzers do not form a DAG.

## Tests

`crates/lighthouse-checks/tests/it`; `cargo test -p lighthouse-checks`.
