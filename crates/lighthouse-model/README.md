# lighthouse-model

The vocabulary every other crate shares: the unified code model (project, symbol, edge, fragment), diagnostics and fingerprints, fix edit types, and the review types (judgment, suppression, label).

**Layer.** L0; depends on protocol only, for its hash. Used by every other crate but protocol and the plugins.

## Entry points
- `Project`, `Symbol`, `Edge`, `Fragment`, `File`: the code model; `Project::merge` joins fragments (`merge.rs`).
- `Diagnostic`, `Fingerprint`, `Severity`, `Incomplete`: what a check reports.
- `Reach`, `Applicability`, `RunScope`: what a rule looks at and what its result depends on.
- `EditOp`, `FixOutcome`, `Safety`, `Preview`: fix vocabulary.
- `Judgment`, `Suppression`, `Label`, `Attribution`: review vocabulary.
- `LineIndex`, `byte_position`, `utf16_position`: column units.

## Invariants
- No I/O, and no rule or plugin knowledge.
- Changing how a `Fingerprint` is computed (it hashes with `lighthouse_protocol::hash`, the only SHA-256 of the workspace) orphans stored findings and judgments and misses every cache.
- The code model is the single in-host shape; the wire mirror lives in protocol and is converted in `rpc::convert`.
- Renaming a model field is not a protocol change (see [docs/architecture.md](../../docs/architecture.md#plugin-kinds-and-manifests)).

## Tests
`crates/lighthouse-model/tests/it`; `cargo test -p lighthouse-model`.
