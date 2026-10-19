# Fixer spec

The `fix:` block of a decision is declarative and selects one fixer *kind*; every kind
compiles into a provider of the single `Fixer` interface, the way declarative rules
compile into `Rule`, and bundled fixers are such specs registered by their plugin through
`Plugin::fixers()` under the decision's id. Local rules in `.lighthouse/decisions/*.yaml` carry
a `fix:` too.

```yaml
fix:
  safety: safe            # safe | unsafe: the cap on what the fixer may claim
  priority: 0             # higher goes first when fixes overlap; default 0
  requires: [extent]      # optional provider capabilities; absent means declined
  # exactly one of:
  ops:                    # CEL over the finding and the code model
    - op: move
      node: finding.evidence.callee
      after: finding.evidence.caller
  command: [gofmt, -s, -w, "{file}"]   # a program (programs.md): no shell
  output: files           # files (the default: what it changed in a scratch copy) | stdout (the new text)
  stdin: none             # none | file; also env, timeout, success, sandbox
  scope: file             # file (default) | symbol | files: where it may change files
  model:                  # an agent rewrites the extent of the finding; see models.md
    prompt: "Rename, do not delete."   # optional: added to the fix prompt
  rpc: {}                 # reserved: a language plugin's own fix method; not yet supported
```

- **`ops`** is a list of generic operations: `move`, `reorder`, `delete`, `rename`,
  `replace`. Each takes typed parameters, validated by the catalog; expressions are CEL over
  `finding` (`rule`, `message`, `file`, `span`, `symbol`, `evidence`, `fingerprint`,
  `facts`), `symbol` (the finding's symbol, as in declarative rules) and `options`, and an
  optional `when` skips an operation. `text` and `name` are templates with `{{ cel }}`
  holes. The operations, their parameters and the registered order keys of `reorder` are
  generated into [decisions/fix-operations.md](decisions/fix-operations.md). Semantics that
  need Rust (`reorder` sorting by a key such as `design/group`, registered by a plugin
  through `Plugin::order_keys()`) live in the operation, not in per-rule code.
- **`model`** is a fix by an agent that edits, not a derivation. A decision has one when it is
  listed in `[fix] decisions` and `[models] agent` is bound, whether it says `fix: {type: model}`
  (optionally with `prompt` and `scope: symbol | files`; undeclared, the decision limits nothing and `[fix] scope` decides) or has a check and no
  fix of its own. Without an `agent` binding the finding stays a review task carrying the brief.
  The agent runs in a scratch copy of the project, once per file; each changed file is a
  whole-file `Replace`, `unsafe` unless the project says `[rules."<id>"] safety = "safe"` for the decision
  (`safety: safe` in the decision itself is refused). The orchestrator
  keeps it only if the scope holds, the finding is gone, nothing of `warn` or `error` is new in the
  changed and referencing files and they are fully analyzed, then writes it (`[fix] unsafe = true`,
  or `--unsafe-fixes`) or reports a preview; `--no-unsafe-fixes` previews it for one run, and
  `[fix] verify` programs must pass over the fixed texts first. Examples need no `fixed` for it.
  See [models.md](models.md#fixes-by-a-model) and [models.md](models.md#the-agent-that-edits).
- **`command`** is a program ([programs.md](programs.md): the shape, the environment, the
  placeholders, the exit codes) run for one finding; structured data goes through `rpc`. Its result
  is changes: `output: files` (the default) takes what it changed in a scratch copy of the project,
  `output: stdout` with `stdin: file` makes it a filter, which writes the new text of `{file}` and
  needs no copy. It goes the way an agent's change goes: one scratch copy, one collection of what
  changed, one scope check and one mapping to the classes (`scope` is `file` unless the decision
  says `symbol` or `files`; a filter is always `file`). A change that stays within the scope is the
  proposal; a run that changed no file declines the finding with what the program said (stderr) as the
  reason; an exit code outside `success`, a signal or a timeout fails it. `{file}`, `{line}`,
  `{symbol}` and `{decision}` fill whole arguments; the finding is in `LIGHTHOUSE_DECISION` as JSON and the
  rule's options in `LIGHTHOUSE_OPTIONS`. The result goes through the same containment, verify and
  rollback as any other fix.
  **Trust:** programs (command fixes and checks, plugins, command providers and `[fix] verify`) run only in a
  project the *user* trusts, whoever wrote them, except those of the bundled catalog.
  `lighthouse trust` lists the programs it would trust and asks for confirmation on the terminal (without a
  terminal it refuses unless `--yes` says the list was read; `init --agent claude-code`
  adds `Bash(lighthouse trust:*)` to the agent's permission deny list); it records the
  canonical root with a digest of `lighthouse.toml`, the rule files and the content of any
  command program that lies inside the project (a program named by a relative path is the
  project's own, run from the project root) in `$XDG_CONFIG_HOME/lighthouse/trust.toml` (default `~/.config/lighthouse/trust.toml`). Programs outside
  the project (`gofmt`, `rustfmt`) are trusted by command line only. Every field of the digest
  is tagged and length-prefixed, and it is computed from the very bytes the configuration
  and rules were loaded from. Authoring candidates (`decision_create`) are never trusted; any change to
  either withdraws it, and `lighthouse trust --revoke` removes it. `LIGHTHOUSE_TRUST=1`
  trusts every project, for CI. The repository cannot grant trust itself: no setting in
  `lighthouse.toml` does (the commands of `[fix] verify` and of command model providers are listed
  by `lighthouse trust` like the others). Without trust a command fix is declined as an
  untrusted project, like every fix that runs a program.
- **Validation** (every catalog load, local layers included): exactly one kind; a decision
  with an implementation; `safe` only on decisions that author `error`; every CEL expression and
  template compiles; operation parameters are well-typed (one of `before`/`after`, `node`
  or `file` and `span`, qualified order keys); a command is a valid program (a program, known placeholders, an output it may have);
  `rename` lists `complete-references`; placeholders fill whole arguments; an `rpc` fix is
  kept but only that rule's findings are declined as not yet supported (it does not fail
  the pack); and the decision has an invalid example with
  `fixed`. The workspace tests also check that every bundled `fix` has its rule and a
  registered fixer, that every `reorder` key is registered and that the `requires` list
  covers what the operations need.
- **Priority.** When fixes overlap in a round, the one with the higher `priority` is taken and the
  others are proposed again in the next round; a tie goes by the order of the findings. Safety is no
  key: a fix the run may not write takes no span, so it never blocks one that is written. Content
  fixes have the default `0`; the formatters have `-100`, so they never win an overlap and
  formatting follows the fix that won.
- **Safe follow-ups.** Whoever makes a fix, a rule, a command or an agent, it is judged on its change
  plus the safe fixes of the findings that change introduced in its files, applied in priority order
  in at most two passes. Those fixes become part of the fix and its preview, so unformatted output of a
  reorder or an agent is formatted and not declined as `introduced`; the rule holds for any safe
  cleanup. In a file that several fixes of a round edit they cannot be told apart, and each is
  reported as an applied fix of its own decision (`follow_up: true`, `(follow-up)` in the text), so
  that every change written belongs to exactly one fix. A preview is made in a round of its own
  where another would edit its file, so it carries the follow-ups of its own change. A finding no safe fix clears still counts against the change. A run that names its own
  `--fixer` settles nothing.
- **The formatter is a decision.** `go/formatted` and `rust/formatted` (severity `error`) check with the
  tool's list mode (`gofmt -l`, `rustfmt --check -l`) and fix with the tool as a filter
  (`stdin: file`, `output: stdout`, `safe`, `priority: -100`), `rust/formatted` with the edition
  of the crate that holds each file (the nearest `Cargo.toml` that sets one, in its package or in
  the workspace it inherits from; `cargo fmt` does the same but needs a manifest at the root and is
  no filter over one file). They are in the recommended sets of
  the language packs and need no trust. To use another formatter, switch the decision off and add a
  decision of the project that names it.
- **Testing:** an invalid example may carry `fixed:`, the new text of each file that
  changes. `lighthouse rule test` applies the fix to the example (unsafe ones too),
  asserts the files equal `fixed` (a final newline aside), checks that the rule no longer
  fires and that applying the fix again changes nothing. `decision_create` and `decision_update`
  run the same examples in their gate.
