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

## Status: phase 1b (Go vertical slice)

Implemented: workspace, unified code model types, plugin contract with
in-process registry (analyzer DAG), `lighthouse.toml` config with presets and
overrides, engine, text/json/SARIF reporters, CLI (`check`, `rule list`,
`explain`, `init`, `docs`) and the bundled plugins:

- `lang-go`: tree-sitter Go provider (`lighthouse-lang-go`, queries in
  `crates/lighthouse-lang-go/queries`), syntactic resolution, no `semantic-edges`.
- `core`: `core/max-file-lines`.
- `metrics`: analyzers `size`, `cyclomatic`, `cognitive`, `nesting`, `fan`,
  language-neutral over the UCM.
- `design`: `design/complexity-signal`, `design/coupling-signal`,
  `design/exported-doc`, `design/single-use-wrapper`.

Control flow reaches analyzers as normalized events: a provider fills
`FunctionSummary::flow` with `FlowKind` constructs (if, else-if, else, switch,
loop, catch, jump, boolean-operator run, recursion), each with its nesting level
(nested functions count as nesting). `cyclomatic` is `decisions + 1`;
`cognitive` follows Campbell, SonarSource 2018, from the events alone.

Pattern catalog (`patterns/`, crate `lighthouse-spec`) is the single source of
truth for rule metadata, option defaults, examples and `docs/patterns/*.md`;
`lighthouse docs generate` rewrites the docs and `lighthouse docs check` fails
when they are stale. `RuleTester` (crate `lighthouse-engine`) runs every
implemented pattern's examples through the engine. `lighthouse rule list --all`
shows every pattern as implemented, unimplemented or doc; `lighthouse explain
<pattern-id>` prints intent, requirement and examples.

Not yet: language servers, SQLite index, MCP/LSP frontends.

## Usage

```
lighthouse init
lighthouse check [paths] [--format text|json|sarif] [--strict] [--rules a,b] [--config file]
lighthouse rule list [--all]
lighthouse docs generate [--out docs]
lighthouse docs check
lighthouse explain core/max-file-lines
```

Exit codes: see `Outcome::exit_code` in `lighthouse-engine`.
