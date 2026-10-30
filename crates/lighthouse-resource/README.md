# lighthouse-resource

The KRM envelope, documents and kind schemas. Layer L0. Never: spec semantics.

The resource model of every spec document: an envelope (`apiVersion`, `kind`, `metadata`, `spec`) over a kind-specific spec, read from YAML, TOML or JSON, written as YAML, and described by a JSON Schema per kind.

**Depends on:** no workspace crate.

## Entry points

- `Spec` (the `KIND` of a content type), `Resource`, `Metadata`: the envelope.
- `documents`, `resource`, `kind_of`, `Format`: reading.
- `to_yaml`: writing.
- `Schema`, `schema`, `schema_file`: one schema per kind, `$id` under `SCHEMA_URL_BASE`.
- `write_atomic`, `write_atomic_guarded`: the one file writer (the guard runs before the rename).
- `parse_duration`, `xdg_plugins`: small shared readers.

## Invariants

- Identity is `metadata.name`; `uid` is assigned once and never derived from the name.
- A document must declare `API_VERSION` and the kind it is read as; anything else is an error.
- No kind is defined here; kinds live in spec, store and rpc.
- `write_atomic` renames a complete temporary file over the target, so readers see either version, and it refuses a symlink.

## Tests

`crates/lighthouse-resource/tests/it`; `cargo test -p lighthouse-resource`.
