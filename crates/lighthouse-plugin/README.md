# lighthouse-plugin

The in-process plugin SPI: the traits and the registry through which rules, analyzers, fixers, language providers and models enter the host.

**Layer.** L1; depends on model and protocol only. Never on spec or on configuration.

## Entry points

- `Plugin`, `Rule`, `Analyzer`, `LanguageProvider`: the traits a plugin implements (bundled code in checks, processes through rpc).
- `Fixer`: fix proposals.
- `Registry`, `plugin_of`: registration, lookup and analyzer resolution.
- `Memo`: values the rules of one run share.
- `Notices`: what a rule tells the user besides its findings.
- `ModelClient`, `Models`, `Ask`, `Settle`, `decide`: asking models and settling their answers.

## Invariants

- Every id is prefixed `<plugin>/` and registered once; analyzers form a DAG, so a missing or cyclic `requires` fails registration.
- It re-exports only the model API types (`SystemOneRequest`, `Question`, `DecisionRef`, ...), never the language wire; wire to model conversion belongs to rpc.
- `fails` and `FAIL_AT` (0.9) are the one policy that turns a model's probability of failing into a judgment (at or above the bound it fails the subject, else it abstains); checks, engine and ml all call them.
- A plugin is a contract boundary, not a process boundary: see [docs/architecture.md](../../docs/architecture.md#plugin-kinds-and-manifests).

## Tests

`crates/lighthouse-plugin/tests/it`; `cargo test -p lighthouse-plugin`.
