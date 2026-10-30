# lighthouse-session

The composition root and the per-command wiring (check, fix, model, signal, hooks logic). Layer L4. Never: terminal I/O.

The operations every frontend shares, and the composition root that wires store, engine, checks, rpc, report and ml: load a project, check and remember it, fix, review, test and author decisions, validate specs, evaluate and train models, write the agent skill.

**Depends on:** `lang-cel`, `lighthouse-checks`, `lighthouse-engine`, `lighthouse-ml`, `lighthouse-model`, `lighthouse-plugin`, `lighthouse-protocol`, `lighthouse-report`, `lighthouse-resource`, `lighthouse-rpc`, `lighthouse-spec`, `lighthouse-store`.

## Entry points

- `Session::load`, `load_in`, `find_in`, `catalog`, `registry`: the configuration, the catalog layers and the plugins of a project, found from a directory without changing the process's.
- `check`, `Checked::render`, `Checked::report`, `fix::run`: run, narrow the report, remember the run.
- `review::{list, show, history, prune, resolve}`: judgments through the store.
- `create_decision`, `replace_decision`, `test_decisions`, `validate_paths`: authoring.
- `model::{eval, train, rebuild, attach, detach, list}`: models (they feed ml and convert store records for it). One store (the first repository's) keeps the datasets and models of a run over several.
- `baseline`: what a hook reports, the findings the agent's session introduced.
- `trust`, `revoke`, `skill`, `bundled_docs`.

## Invariants

- Frontends call these functions and print; they hold no logic. The same judgment recorded through the CLI, MCP or a hook is the same record.
- Trust is the user's decision, never the repository's: it is a digest of the configuration, rules and in-project programs as loaded, and any change withdraws it. Commands (formatters, command fixers) run only in a trusted project.
- Report scope (git changes, since) narrows the report, never the analysis.
- An authored decision is written to `.lighthouse/decisions` only after its candidate layer loads and all its examples pass.
- The lib re-exports the types frontends need (including the ml request and result types), so a frontend imports no other workspace crate.

## Tests

No crate tests of its own: exercised end to end by `crates/lighthouse-cli/tests/it`; `cargo test -p lighthouse-cli`.
