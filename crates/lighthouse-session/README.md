# lighthouse-session

The operations every frontend shares, and the composition root that wires store, engine, checks, rpc, report and ml: load a project, check and remember it, fix, review, test and author decisions, validate specs, evaluate and train models, write the agent skill.

**Layer.** L4; depends on every layer below except protocol and process.

## Entry points
- `Session::load`, `catalog`, `registry`: the configuration, the catalog layers and the plugins of a project.
- `check`, `fix`: run, narrow the report, remember the run.
- `review_finding`, `record_judgment`, `review_tasks`: judgments through the store.
- `create_decision`, `replace_decision`, `test_decisions`, `validate_paths`: authoring.
- `eval`, `train`, `bound_models`: models (they feed ml and convert store records for it).
- `trust`, `revoke`, `skill`, `bundled_docs`.

## Invariants
- Frontends call these functions and print; they hold no logic. The same judgment recorded through the CLI, MCP or a hook is the same record.
- Trust is the user's decision, never the repository's: it is a digest of the configuration, rules and in-project programs as loaded, and any change withdraws it. Commands (formatters, command fixers) run only in a trusted project.
- Report scope (git changes, since) narrows the report, never the analysis.
- An authored decision is written to `.lighthouse/decisions` only after its candidate layer loads and all its examples pass.
- The lib re-exports the types frontends need (including the ml request and result types), so a frontend imports no other workspace crate.

## Tests
No crate tests of its own: exercised end to end by `crates/lighthouse-cli/tests/it`; `cargo test -p lighthouse-cli`.
