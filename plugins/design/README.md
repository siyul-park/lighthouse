# design

The bundled declarative plugin of the design decisions: naming, functions, types and APIs, errors, comments, file order, dependency direction and concurrency. Its pack `design` is in `decisions/design`, with the source map of the decisions (`sources.yaml`) and the labels the local models learn from (`labels/`); the code that runs its checks is in `lighthouse-checks`.

**Layer.** Bundled: it has no process, the binary embeds `decisions/` and compiles the checks in.

## Entry points

- `lighthouse-plugin.toml`: the manifest; `catalog = "decisions"` names the catalog.
- `decisions/design/pack.yaml`: the pack and its sections.
- `decisions/design/sources.yaml`: the lines of the coding-patterns document the decisions answer, or why one is omitted.
- `labels/<name>.tsv`: reviewed findings per decision (`fingerprint repo location verdict reason`).

## Invariants

- Decision ids (`design/<name>`) and uids never change.
