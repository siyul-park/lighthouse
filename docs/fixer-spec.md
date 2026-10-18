# Fixer spec

The `fix:` block of a decision is declarative and selects one fixer *kind*; every kind
compiles into a provider of the single `Fixer` interface, the way declarative rules
compile into `Rule`, and bundled fixers are such specs registered by their plugin through
`Plugin::fixers()` under the decision's id. Local rules in `.lighthouse/decisions/*.yaml` carry
a `fix:` too.

```yaml
fix:
  safety: safe            # safe | unsafe: the cap on what the fixer may claim
  requires: [extent]      # optional provider capabilities; absent means declined
  # exactly one of:
  ops:                    # CEL over the finding and the code model
    - op: move
      node: finding.evidence.callee
      after: finding.evidence.caller
  command:
    argv: ["gofmt", "-s", "-w", "{file}"]   # no shell
    output: inPlace       # inPlace | text (the new file text on stdout)
    stdin: none           # none | file
    env: {}               # extra variables; the rest is cleared
    timeout: 30s
    scope: file           # edits outside the finding's file are refused
  model:                  # a model rewrites the extent of the finding; see models.md
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
  listed in `[fix] decisions` and `[models] edit` is bound, whether it says `fix: {type: model}`
  (optionally with `prompt` and `scope: symbol | files`; undeclared, the decision limits nothing and `[fix] scope` decides) or has a check and no
  fix of its own. Without an `edit` binding the finding stays a review task carrying the brief.
  The agent runs in a scratch copy of the project, once per file; each changed file is a
  whole-file `Replace`, `unsafe` and never `safe` (`safety: safe` is refused). The orchestrator
  keeps it only if the scope holds, the finding is gone, nothing of `warn` or `error` is new in the
  changed and referencing files and they are fully analyzed, then writes it (`[approval] fix =
  "allow"`) or reports a preview (`"ask"`); `--unsafe-fixes` writes it whatever the approval, and
  `[fix] verify` commands must pass over the fixed texts first. Examples need no `fixed` for it.
  See [models.md](models.md#fixes-by-a-model) and [models.md](models.md#the-agent-that-edits).
- **`command`** is the simple text contract; structured data goes through `rpc`. It runs a
  program without a shell in a scratch directory that holds a copy of the finding's file.
  *Exit codes:* `0` succeeds with the result per `output` (an empty result changes nothing),
  `1` declines with the reason taken from stderr, `>= 2`, a signal or a timeout is an error:
  nothing is applied and the capped stderr goes into the notice. *Output:* `inPlace` diffs
  the scratch copy against the original into the smallest whole-line `Replace`; `text` takes
  the new file text from stdout, which carries only the result (stderr is never parsed).
  *stdin:* `none` (default) or `file`, the target file's content; no request JSON is sent.
  *Argv:* `{file}`, `{line}`, `{symbol}` and `{rule}` fill a whole argument each (no
  embedding, no `{root}`; a value starting with `-` is refused); the working directory is the
  scratch directory, and a symlink in the output is refused. *Environment:* cleared to
  `PATH`, `HOME`, `LANG`, `TMPDIR` (a directory of the run's own), plus the declared `env`, plus `LIGHTHOUSE_DECISION` (the
  finding as JSON), `LIGHTHOUSE_OPTIONS` (the rule's options as JSON) and
  `LIGHTHOUSE_API_VERSION`. *Limits:* a timeout sends SIGTERM to the process group, then
  SIGKILL after a grace period, through the runner shared with the plugin host
  (`lighthouse-process`); stdout and stderr are capped. The result goes through the same
  containment, verify and rollback as any other fix.
  **Trust:** commands (command fixers and `[languages.<id>] formatter`) run only in a
  project the *user* trusts, whoever wrote them. `lighthouse trust` lists the formatters and
  fixer commands it would trust and asks for confirmation on the terminal (without a
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
  untrusted project, like every fix that runs a program, and formatting is skipped with a note.
- **Validation** (every catalog load, local layers included): exactly one kind; a decision
  with an implementation; `safe` only on decisions that author `error`; every CEL expression and
  template compiles; operation parameters are well-typed (one of `before`/`after`, `node`
  or `file` and `span`, qualified order keys); a command has a program and a timeout;
  `rename` lists `complete-references`; placeholders fill whole arguments; an `rpc` fix is
  kept but only that rule's findings are declined as not yet supported (it does not fail
  the pack); and the decision has an invalid example with
  `fixed`. The workspace tests also check that every bundled `fix` has its rule and a
  registered fixer, that every `reorder` key is registered and that the `requires` list
  covers what the operations need.
- **Testing:** an invalid example may carry `fixed:`, the new text of each file that
  changes. `lighthouse rule test` applies the fix to the example (unsafe ones too),
  asserts the files equal `fixed` (a final newline aside), checks that the rule no longer
  fires and that applying the fix again changes nothing. `decision_create` and `decision_update`
  run the same examples in their gate.
