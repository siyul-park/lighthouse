# lighthouse-model

The vocabulary every other crate shares: the code model (UCM: project, symbol, edge, fragment) and the fix edit ops over it in `ucm`, diagnostics and fingerprints, the fix preview, and the review types (judgment, suppression, label).

**Layer.** L0; depends on protocol only, for its hash. Used by every other crate but protocol and the plugins.

## Entry points
- `Project`, `Symbol`, `Edge`, `Fragment`, `File`: the code model (UCM); `Project::merge` joins fragments (`ucm/merge.rs`); `Project::discriminator` tells apart findings that share a fingerprint.
- `Diagnostic`, `Fingerprint`, `Severity`, `Incomplete`: what a check reports.
- `Reach`, `RunScope`, `TestScope`: what a rule looks at and what its result depends on; `Applicability` (in `ucm`) decides which code-model subjects a decision applies to.
- `ucm::{EditOp, Anchor, Owner, FixOutcome}`: the edits a fixer proposes over code-model nodes; `Safety`, `Preview`: the fix vocabulary that is not about code.
- `Position`, `Span`: text positions, shared by diagnostics and the code model.
- `Judgment`, `Suppression`, `Label`, `Attribution`: review vocabulary.
- `LineIndex`, `byte_position`, `utf16_position`: column units.

## Invariants
- No I/O, and no rule or plugin knowledge.
- Changing how a `Fingerprint` is computed (it hashes with `lighthouse_protocol::hash`, the only SHA-256 of the workspace) orphans stored findings and judgments and misses every cache.
- The core modules (everything but `ucm`) know nothing of the code model, and neither do `store`, `ml`, `report` and `session`: a `forbidden` contract of `design/layers` enforces it.
- The code model is the single in-host shape; the wire mirror lives in protocol and is converted in `rpc::convert`.
- Renaming a code-model field is not a protocol change (see [docs/architecture.md](../../docs/architecture.md#plugin-kinds-and-manifests)).

## Tests
`crates/lighthouse-model/tests/it`; `cargo test -p lighthouse-model`.
