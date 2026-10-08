# Lighthouse

**Lighthouse continuously turns codebase design decisions into executable rules.**

Every codebase runs on design decisions: which way dependencies point, who owns which
state, what a test may touch, when a helper deserves to exist. Most of them live in
review threads, style guides and people's heads. So they get made again, argued again
and broken again, by humans and by coding agents that forget everything when the
session ends.

Lighthouse remembers them. It records each decision with its reasons, applies it to
every later change, and turns the decisions that keep coming back into rules.

```text
 code ─▶ check ─▶ finding ─▶ review ─▶ verdict ─▶ memory
                                                    │
        every change ◀── rule ◀── rule proposal ◀── recurring decision
```

It is not another multi-language linter and not an AI reviewer. What it adds is time:
**design decisions remembered across every change, by every person and agent who touches
the code.**

## Decision Memory

A finding is a question: *does this code follow the decision?* When the answer is "yes,
on purpose", that answer is kept, so nobody is asked again.

```sh
lighthouse review resolve 395d1985afe8 --verdict rejected --reason intentional-exception \
  --note "composition root wires every service"
```

- **Shared, not local.** Verdicts are appended to `.lighthouse/decisions.jsonl`, which
  is committed with the code (`merge=union`, so branches merge cleanly). Teammates and
  CI see the same decisions.
- **At the code, when it belongs there.** A decision about one place can be written
  where it happens and reviewed in the diff:
  `// lighthouse:allow design/coupling-signal -- composition root wires every service`.
  Annotations that stop matching anything are reported, so they cannot rot.
- **Valid while it still applies.** A verdict holds while the rule's meaning and the
  finding's evidence stay the same. Change the code materially or redefine the rule, and
  the question comes back. Mechanical findings are never hidden by a verdict, only by an
  annotated, reviewed exception; heuristic and judgment findings can be, at any severity.
- **Evidence kept.** Every verdict stores a snapshot of what was judged: metrics, symbol
  shape, rule options, commit. That history is what later turns decisions into rules.

## Decision Catalog

Each decision has one canonical specification. The checks, the rendered docs
([docs/decisions](docs/decisions)) and the guidance agents read are all generated from it,
so docs, linter and agent cannot disagree.

```yaml
apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: design/single-use-wrapper
  labels: { lighthouse/pack: design, lighthouse/section: functions }
spec:
  title: Inline single-use wrappers
  intent: A forwarding wrapper adds a name without adding meaning.
  scope: { domain: code, subject: symbol }
  requirement: >-
    A simple single-use wrapper SHOULD be inlined unless its name expresses a
    real policy or mechanic.
  enforcement: heuristic     # mechanical → error · heuristic → warn · judgment → info
  check: { type: builtin, id: design/single-use-wrapper }
  examples: [...]            # executable valid/invalid fixtures, per language
```

A decision states its intent once; each language realizes it in its own terms. Rules can
be built in, written as CEL expressions over the code model, or added per project under
`.lighthouse/decisions/`. Every decision ships executable examples, and `lighthouse decision test`
runs them. Every spec document, `lighthouse.toml` included, has a JSON Schema in [schema/](schema).

Precision comes before recall. A design rule that cries wolf gets ignored, so heuristics
default to high thresholds, judgment calls become `info` review tasks instead of errors, and
each rule's precision is measured from its verdicts.

## Rule Evolution

When structurally similar code keeps receiving the same verdict and no rule covers it,
or a rule keeps being rejected in the same way, Lighthouse proposes a change: a new rule,
a wider one, or a narrower one. A proposal carries the occurrences, the verdicts and
generated valid/invalid examples. It must pass every existing fixture before anyone
approves it. Nothing is enabled automatically.

Rules aim to be deterministic and explicit. Statistical models and decision models
assist, by finding similar code, estimating confidence and ranking proposals, but they
do not replace the rule. The same evaluation pipeline is exposed to agents, so an agent
writing a rule tests it exactly the way Lighthouse does.

## In an agent's loop

A hook runs `lighthouse check` on what the agent just changed and returns feedback the
agent can act on:

```text
design/private-helper-callers  info (heuristic)  src/lib.rs:5:1
  owner:       demo::clamp#function
  message:     private function clamp has one caller (run)
  requirement: A private helper SHOULD have at least two callers.
  intent:      A private helper with one caller is usually part of that caller.
  evidence:    caller=demo::run#function callers=1 statements=3
  expected:    canonical rust example (src/lib.rs)
                 pub fn run(x: u8) -> u8 { x.min(9) + 1 }
  fingerprint: 395d1985afe8
```

The agent fixes it, or records why not. Either way the decision is kept.

## Why not another linter

| | Linters | Agent-time scanners | Lighthouse |
| --- | --- | --- | --- |
| Checks code against rules | yes | yes | yes |
| Runs while an agent writes | no | yes | yes |
| Design-level structure (ownership, dependency direction, cohesion, test contracts) | rarely | rarely | the focus |
| Remembers every review decision, shared through git | no | no | yes |
| Turns recurring decisions into new or tighter rules | no | no | yes |
| Rules, docs and agent guidance from one source | no | no | yes |

## Quick start

```sh
cargo install --path crates/lighthouse-cli   # the `lighthouse` binary
make plugins                                  # language plugins into target/plugins/

lighthouse init                               # writes lighthouse.toml
lighthouse check                              # analyze the project
lighthouse check --changed --format agent     # what the working tree changed, for an agent
lighthouse explain design/single-use-wrapper  # intent, requirement, examples
lighthouse review list                        # findings Lighthouse remembers
lighthouse decision test                      # run every decision's examples
```

A minimal `lighthouse.toml` for a Go project:

```toml
apiVersion = "lighthouse/v1alpha1"
kind = "Project"
metadata = { name = "demo" }

[spec]
plugins = [{ id = "lang-go", path = "target/plugins/lang-go" }, "design"]
extends = ["design/recommended"]

[spec.rules]
"design/complexity-signal" = { level = "warn", options = { cognitive = 30 } }
```

Exit codes: `0` clean, `1` an error (a warning too with `--strict` or `--max-warnings N`), `2` usage or configuration error, `3` analysis
incomplete. "Not checked" never counts as "passed".

## Built from plugins

The core only coordinates. Every capability is a plugin kind with one interface and
any number of providers: languages, analyzers, rules, presets, and next fixers and
judges. Bundled capabilities use the same contracts as third-party ones. A plugin is a
contract boundary, not necessarily a process: language plugins run as separate
processes written in their own language (the Go plugin uses `go/packages` and
`go/types`) and speak a small [JSON-RPC protocol](docs/plugin-protocol.md), so supporting
a new language never requires Rust.

Code is the first domain. The engine is built around artifacts and decisions, so
documents and design files can follow.

## Status

| Area | Available now | Next |
| --- | --- | --- |
| Decision Memory | committed decision log, expiring verdicts with evidence snapshots, source annotations, finding history | MCP tools for agents, hooks |
| Decision Catalog | `design` and `testing` packs, generated docs, JSON Schemas, SARIF, project overrides, CEL checks, `decision test` | more executable examples |
| Rule Evolution | | decision index and similarity, coverage analysis, rule proposals |
| Rules | complexity, coupling, docs, wrappers, declaration order and layout, naming, banners, test contracts | dependency direction, cohesion, clones |
| Languages | Go (semantic), Rust (syntactic) | TypeScript, Python |
| Fixing | | one fixer interface: rule-based fixes first, then plugin and agent fixers |
| Editors | | LSP server |

## Documentation

- [Decision catalog, rendered](docs/decisions): every decision Lighthouse knows
- [Architecture](docs/architecture.md): code model, scopes, incomplete analysis, memory and shared decisions
- [Plugin protocol](docs/plugin-protocol.md): writing a language plugin in any language
- [Roadmap](docs/roadmap.md): what is done, what comes next, and why

Development: `make test` builds the plugins and runs the test suites, `make lint` runs
formatters, clippy, `go vet` and Lighthouse on its own sources, and `make ci` runs both.
