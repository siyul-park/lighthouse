# Lighthouse

**Lighthouse continuously turns codebase design decisions into executable rules.**

Every codebase accumulates design decisions: which way dependencies point, who owns
which state, what a test is allowed to touch, when a helper deserves to exist. Most of
them live in review threads, style docs and people's heads, so they are made again and
again, and broken again and again, by humans and coding agents alike.

Lighthouse records those decisions, turns them into rules that can be replayed against
any change, and applies them to every decision that follows.

```text
            ┌──────────────────────── decide once ────────────────────────┐
            │                                                             │
   change ──▶ check ──▶ finding ──▶ review ──▶ verdict ──▶ pattern memory ─┤
     ▲          │      (evidence)   (agent      (kept       (similar code, │
     │          │                    or human)   forever)    past verdicts)│
     │          ▼                                                          ▼
     └──── fix ◀── feedback                         rule proposal ──▶ rule test ──▶ new rule
                                                                                      │
            ◀──────────────────────── apply everywhere ───────────────────────────────┘
```

## System 1 for your agents

A coding agent reasons slowly and expensively, and forgets what it concluded once the
session ends. Lighthouse is the fast, cheap, reproducible layer underneath: the
judgments the agent (or a reviewer) already made, compiled into rules that run in
milliseconds on every edit. The agent stays System 2, deliberate and creative.
Lighthouse is System 1: the codebase's trained intuition.

Every judgment that System 2 makes is stored, and the ones that keep recurring are
promoted into System 1.

## What the loop looks like

An agent edits a file. A hook runs `lighthouse check` on what changed and hands back
structured feedback rather than a bare lint line (agent output format, arriving with the MCP and hook integration):

```text
internal/jit/compile/compile.go:350:1: warn design/coupling-signal:
  function coordinates 14 collaborators in its package
  evidence: fan_in=2 fan_out=14 statements=61
  requirement: coupling is judged from fan-in and fan-out within a package (review signal)
```

The agent fixes it, or it records a verdict: `rejected: intentional-exception` with a
reason. That verdict is kept with a snapshot of the evidence, so the same structure is
never asked about twice. When structurally similar code keeps getting the same
verdict and no rule covers it, Lighthouse proposes one. The proposal carries the
occurrences, the verdicts and valid/invalid examples. It must pass the existing rule
fixtures before anyone approves it.

Rules come from a **pattern catalog**: one canonical specification per decision, from
which the checks, the human-readable docs ([docs/patterns](docs/patterns)) and the
agent instructions are all generated. Docs, linter and agent can no longer disagree.

```yaml
id: design/single-use-wrapper
title: Inline single-use wrappers
intent: A forwarding wrapper adds a name without adding meaning.
scope: symbol
requirement: >-
  A simple single-use wrapper SHOULD be inlined unless its name expresses a
  real policy or mechanic; complex one-use mechanics require review rather
  than mechanical removal.
enforcement: heuristic     # mechanical → error · heuristic → warn · judgment → review
evidence: [callee, callers]
implementation:
  builtin: design/single-use-wrapper
examples: [...]            # executable valid/invalid fixtures, run by RuleTester
```

## How a decision becomes a rule

A decision can be enforced in three forms. A pattern climbs from the left to the right
as evidence accumulates, and every step is gated by offline evaluation on held-out
verdicts and by the rule's fixtures.

| Form | How it judges | When it is used |
| --- | --- | --- |
| **judged** | the requirement in natural language, evaluated by a pluggable decision model | the decision is real but nobody can write it down precisely yet |
| **learned** | a gradient-boosted tree model trained on stored verdicts and feature snapshots | enough labelled cases exist and the boundary is statistical |
| **deterministic** | a mechanical check or a heuristic expression over the code model | the boundary can be stated exactly; this is the goal of every pattern |

Rules define what is true. Models only estimate confidence and priority. Nothing is
enabled, suppressed or promoted without passing the same evaluation pipeline, and that
pipeline is exposed to agents through the CLI and MCP, so an agent writing a new rule
evaluates it exactly the way Lighthouse does.

## Why not another linter

| | Formatters and linters | Agent-time scanners | Lighthouse |
| --- | --- | --- | --- |
| Checks code against rules | yes | yes | yes |
| Runs while an agent writes | no | yes | yes |
| Design-level structure (ownership, dependency direction, cohesion, test contracts) | rarely | rarely | the focus |
| One rule set across languages, realized per language | no | partly | yes |
| Remembers every review verdict | no | no | yes |
| Turns recurring verdicts into new or tighter rules | no | no | yes |
| Rules, docs and agent instructions share one source | no | no | yes |

Precision comes before recall. A design rule that cries wolf gets ignored, so heuristic
rules default to high thresholds, judgment calls become review tasks instead of
errors, and each rule's precision is measured from its verdicts. Noisy rules generate
proposals to narrow or demote themselves.

## Quick start

```sh
cargo install --path crates/lighthouse-cli   # the `lighthouse` binary
make plugins                                  # builds language plugins into target/plugins/

lighthouse init                               # writes lighthouse.toml
lighthouse check                              # analyze the project
lighthouse explain design/single-use-wrapper  # intent, requirement, examples
lighthouse rule list --all                    # every pattern and its status
```

A minimal `lighthouse.toml` for a Go project:

```toml
plugins = [{ id = "lang-go", path = "target/plugins/lang-go" }, "design"]
extends = ["design/recommended"]

[rules]
"design/complexity-signal" = { level = "warn", cognitive = 30 }
```

Exit codes: `0` clean, `1` findings, `2` usage or configuration error, `3` analysis
incomplete. "Not checked" never counts as "passed".

## Concepts

- **Pattern catalog** (`patterns/`): the canonical design decisions, with intent,
  requirement, scope, enforcement tier, options, per-language tuning and executable
  examples. Packs today: `design` and `testing`. Projects can overlay their own.
- **Unified code model**: modules, symbols, edges, control-flow summaries and test
  cases. Rules state their intent once, and each language realizes it in its own
  terms.
- **Language plugins**: each language is analyzed by a separate process written in
  that language, using its own toolchain. The Go plugin uses `go/packages` and
  `go/types`. Plugins speak a small, versioned
  [stdio JSON-RPC protocol](docs/plugin-protocol.md), so a provider for a new
  language never needs Rust.
- **Analysis scope versus report scope**: `check [paths]`, `--changed` and hook
  scopes filter what is reported. Analysis always covers what the semantics require.
  See [docs/architecture.md](docs/architecture.md).
- **Beyond code**: the engine is built around artifacts and decisions, not syntax.
  Source code is the first domain; documents, design files and other stored artifacts
  are next.

## Status

| Area | Available now | Next |
| --- | --- | --- |
| Pattern catalog | `design` and `testing` packs from a real style guide, generated docs, overlays | more executable examples |
| Languages | Go (semantic), Rust (syntactic), both over RPC | TypeScript, Python |
| Analysis | size, cyclomatic, cognitive (SonarSource), nesting, fan-in/out | dependency direction, cycles, clones, cohesion |
| Rules | complexity, coupling, exported docs, single-use wrappers | declarative CEL rules, test-contract rules |
| Agent loop | CLI with text, JSON and SARIF output | agent output format, MCP server, Skill, hooks |
| Memory | | verdict store, pattern index, similarity search |
| Evolution | | coverage analysis, rule proposals, judged and learned rule forms |
| Editors | | LSP server |

## Documentation

- [Pattern catalog, rendered](docs/patterns): every decision Lighthouse knows
- [Architecture](docs/architecture.md): core model, scopes and incomplete analysis
- [Plugin protocol](docs/plugin-protocol.md): writing a language plugin in any language

Development: `make test` builds the plugins and runs the test suite. The Go plugin
pins its toolchain in `plugins/lang-go/.go-version`.
