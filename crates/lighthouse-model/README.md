# lighthouse-model

The vocabulary every other crate shares: the unified code model (project, symbol, edge, fragment), diagnostics and fingerprints, fix edit types, and the review types (judgment, suppression, label).

**Layer.** L0; depends on no workspace crate. Used by every other crate.

## Entry points
- `Project`, `Symbol`, `Edge`, `Fragment`, `File`: the code model; `Project::merge` joins fragments (`merge.rs`).
- `Diagnostic`, `Fingerprint`, `Severity`, `Incomplete`: what a check reports.
- `Reach`, `Applicability`, `RunScope`: what a rule looks at and what its result depends on.
- `EditOp`, `FixOutcome`, `Safety`, `Preview`: fix vocabulary.
- `Judgment`, `Suppression`, `Label`, `Attribution`: review vocabulary.
- `hash`: the one SHA-256 of the workspace; `canonical`: canonical JSON.
- `LineIndex`, `byte_position`, `utf16_position`: column units.

## Invariants
- No I/O, and no rule or plugin knowledge.
- `hash` is the only SHA-256: content hashes, fingerprints, record ids and trust bases all use it. Changing how a `Fingerprint` is computed orphans stored findings and judgments and misses every cache.
- The code model is the single in-host shape; the wire mirror lives in protocol and is converted in `rpc::convert`.
- Renaming a model field is not a protocol change (see [docs/architecture.md](../../docs/architecture.md#plugin-kinds-and-manifests)).

## Tests
`crates/lighthouse-model/tests/it`; `cargo test -p lighthouse-model`.
