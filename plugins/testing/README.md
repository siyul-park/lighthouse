# testing

The bundled declarative plugin of the testing decisions: the public boundary, test organization and evidence. Its pack `testing` is in `decisions/testing`, with the source map of its decisions (`sources.yaml`) and the labels the local models learn from (`labels/`); the code that runs its checks is in `lighthouse-checks`.

**Layer.** Bundled: it has no process, the binary embeds `decisions/` and compiles the checks in.

## Entry points

- `lighthouse-plugin.toml`: the manifest; `catalog = "decisions"` names the catalog.
- `decisions/testing/pack.yaml`: the pack and its sections.
- `decisions/testing/sources.yaml`: the lines of the coding-patterns document the decisions answer, or why one is omitted.
- `labels/<name>.tsv`: reviewed findings per decision.

## Invariants

- Decision ids (`testing/<name>`) and uids never change.
