# Lighthouse

Design-quality linter written in Rust. One rule set across languages, delivered
as a feedback loop for agents and humans. The core knows zero languages and
zero rules: languages, analyzers, rules and presets all arrive through one
`Plugin` contract, and bundled plugins use the same contract as external ones.

## Architecture

```
 frontends:  CLI ── LSP server ── MCP server ── Skill/hooks
                 \      |          /
                  engine (orchestrator)
     config ─ plugin host ─ scheduler ─ baseline/ratchet ─ reporters
        |                       |
   language providers      analyzer DAG ──> rules ──> diagnostics / review tasks
   (tree-sitter + LSP)      (metrics, facts)              |
        \_____________ index (SQLite, .lighthouse/) ______/
                       files·symbols·edges·metrics·findings history·snapshots
```

## Status: phase 1 (scaffold)

Implemented: workspace, unified code model types, plugin contract with
in-process registry (analyzer DAG), `lighthouse.toml` config with presets and
overrides, engine, text/json/SARIF reporters, CLI (`check`, `rule list`,
`explain`, `init`) and the bundled `core` plugin (`core/max-file-lines`).

Pattern catalog (`patterns/`, crate `lighthouse-spec`) is the single source of
truth for rule metadata and `docs/patterns/*.md`; `lighthouse docs generate`
rewrites the docs and `lighthouse docs check` fails when they are stale.
`lighthouse rule list --all` shows every pattern as implemented, unimplemented
or doc; `lighthouse explain <pattern-id>` prints intent, requirement and examples.

Not yet: tree-sitter, language servers, SQLite index, MCP/LSP frontends.

## Usage

```
lighthouse init
lighthouse check [paths] [--format text|json|sarif] [--strict] [--rules a,b]
lighthouse rule list [--all]
lighthouse docs generate [--out docs]
lighthouse docs check
lighthouse explain core/max-file-lines
```

Exit codes: see `Outcome::exit_code` in `lighthouse-engine`.
