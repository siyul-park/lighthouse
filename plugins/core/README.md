# core

The bundled declarative plugin of the language-independent decisions: size limits, source annotations, decision naming and the formats of configuration files. Its pack `core` is in `decisions/core`; the code that runs its checks is in `lighthouse-checks`, and the plugin also provides the plain-text fallback language.

**Layer.** Bundled: it has no process, the binary embeds `decisions/` and compiles the checks in.

## Entry points

- `lighthouse-plugin.toml`: the manifest; `catalog = "decisions"` names the catalog.
- `decisions/core/pack.yaml`: the pack and its sections; one directory per section holds the decisions.

## Invariants

- Decision ids (`core/<name>`) and uids never change: findings, judgments and docs links name them.
