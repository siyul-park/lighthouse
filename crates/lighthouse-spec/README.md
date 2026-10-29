# lighthouse-spec

The spec kinds (Decision, Pack, Project, check and fix specs, options schema) and the `Catalog` that loads, layers and validates them.

**Layer.** L1; depends on model, protocol and resource. Never on plugin: checks bind to decisions by id string.

## Entry points

- `Catalog`: `bundled()`, `load`, `overlay` (the project layer over the bundled one), `load_local`.
- `Decision`, `Pack`, `Section`: the documents of the catalog; `Decision::reference()` gives the protocol `DecisionRef`.
- `Check`, `CheckKind` (cel, command, rpc, model) and `Fix`, `FixKind`: how a decision is checked and fixed.
- `Config`: `lighthouse.toml` (rule levels, overrides, plugins, languages, model bindings).
- `descriptors()`: the JSON Schema of every kind defined here.

## Invariants

- The bundled catalog is compiled into the binary and must load; a decision that fails validation is an `Error`, never skipped.
- Spec only describes; the bridge from a decision to a `Rule` lives in checks.
- Kinds live here, their envelope and formats in resource; docs rendering lives in report.
- A decision's meaning version hashes its requirement, severity, scope and options, never its `check`; changing those stops judgments from standing.

## Tests

`crates/lighthouse-spec/tests/it`, fixtures in `tests/sources`; `cargo test -p lighthouse-spec`.
