# lighthouse-model

Every data shape Lighthouse speaks, and how a document is read and stored. Layer L0. Never: processes, network, the store.

The vocabulary every other crate shares, and the resource model of every spec document: the code model (UCM: project, symbol, edge, fragment) and the fix edit ops over it in `ucm`, diagnostics and fingerprints, the fix preview, and the review types (judgment, suppression, label); the KRM envelope, its formats, schemas, durations and the files they are kept in.

**Depends on:** `lighthouse-protocol`.

## Entry points

- `ucm::{Project, Symbol, Edge, Fragment, File, …}`: the code model (UCM), imported from `lighthouse_model::ucm` only; `Project::merge` joins fragments (`ucm/merge.rs`); `Project::discriminator` tells apart findings that share a fingerprint.
- `Diagnostic`, `Fingerprint`, `Severity`, `Incomplete`: what a check reports.
- `Reach`, `RunScope`, `TestScope`: what a rule looks at and what its result depends on; `Applicability` (in `ucm`) decides which code-model subjects a decision applies to.
- `ucm::{EditOp, Anchor, Owner, FixOutcome}`: the edits a fixer proposes over code-model nodes; `Safety`, `Preview`: the fix vocabulary that is not about code.
- `Position`, `Span`: text positions, shared by diagnostics and the code model.
- `Judgment`, `Suppression`, `Label`, `Attribution`: review vocabulary.
- `LineIndex`, `byte_position`, `utf16_position`: column units.
- `Spec` (the `KIND` of a content type), `Resource`, `Metadata`: the envelope; `new_uid`, `is_uid`: identity.
- `Format`, `documents`, `resource`, `kind_of`, `to_yaml`: reading YAML, TOML and JSON, writing YAML. `Document::list` finds the Lighthouse documents of a text and where each name is written.
- `Schema`, `schema`, `schema_file`, `header`: one JSON Schema per kind, `$id` under `SCHEMA_URL_BASE`.
- `Duration`, `now`: time.
- `write_atomic`, `write_atomic_guarded`, `read_optional`, `file_in`, `xdg_dir`, `xdg_plugins`: where files are and the one file writer (the guard runs before the rename); `var`, `var_os`, `replace_vars`: the one lookup of the user's variables.

## Invariants

- No process, no network, no store, and no rule or plugin knowledge; the only I/O is reading and writing documents and user-level files.
- Identity is `metadata.name`; `uid` is assigned once and never derived from the name. A document must declare `API_VERSION` and the kind it is read as; anything else is an error. No kind is defined here: kinds live in spec, store and rpc.
- `write_atomic` renames a complete temporary file over the target, so readers see either version, and it refuses a symlink.
- User variables (`XDG_*`, `HOME`, `USER`, trust and attribution) are read through `var`/`var_os` only; a process that opens sessions itself (a test) replaces some of them once with `replace_vars` instead of changing the environment other threads read. Explicit injection is `xdg_dir_in`.
- Changing how a `Fingerprint` is computed (it hashes with `lighthouse_protocol::hash`, the only SHA-256 of the workspace) orphans stored findings and judgments and misses every cache.
- The core modules (everything but `ucm`) know nothing of the code model, and neither do `store`, `ml`, `report` and `session`: a `forbidden` contract of `design/layers` enforces it.
- The code model is the single in-host shape; the wire mirror lives in protocol and is converted in `rpc::convert`.
- Renaming a code-model field is not a protocol change (see [docs/architecture.md](../../docs/architecture.md#plugin-kinds-and-manifests)).

## Tests

`crates/lighthouse-model/tests/it`; `cargo test -p lighthouse-model`.
