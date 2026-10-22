# Programs

Lighthouse runs external programs at four places: the check of a decision, the fix of a decision, the programs of `[fix] verify` and the command of a model provider. They are one thing, a **program**, with one shape, one way to run, one environment and one way to read the result. What differs between the places is which placeholders the command line may hold, which outputs make sense and when the program runs, and that is all the place decides.

## The shape

One schema, `$defs/program` in [decision.schema.json](../schema/decision.schema.json), is read at every place. A bare list of strings is a program that has only a `command`.

```toml
command = ["golangci-lint", "run", "--output.sarif.path", "/dev/stdout", "{files}"]
env     = { GOFLAGS = "-mod=mod", NVIDIA_API_KEY = "${NVIDIA_API_KEY}" }
timeout = "2m"
stdin   = "none"      # none | file
output  = "sarif"     # lines | sarif | status | files | stdout
success = [0, 1]
sarif   = { rules = ["errcheck", "govet:*"], levels = ["error", "warning"], columns = "utf16CodeUnits" }
sandbox = { network = ["proxy.golang.org"], write = ["~/.cache/golangci-lint"] }
```

| Field | Meaning | Default |
| --- | --- | --- |
| `command` | The program and its arguments, run without a shell. | required |
| `env` | Variables besides the one allowlist ([environment](#the-environment)). A value may hold `${NAME}`: the user's variable ([credentials](#credentials)). | none |
| `timeout` | How long it may run. SIGTERM, a grace period, then SIGKILL, for the whole process group. | `2m`; `10m` for an agent and for a `[fix] verify` program |
| `stdin` | `none`, or `file`: the text of `{file}`. | `none` |
| `output` | What it prints, which decides the result ([results](#results)). | `lines` for a check, `files` for a fix, `status` for a verify program, `stdout` for a provider |
| `success` | The exit codes that mean the program ran. Any other code, a signal or a timeout is an error, never a clean result. | `[0, 1]` for `lines` and `sarif` (linters exit 1 with findings), `[0]` otherwise |
| `sarif` | How a SARIF log is read, under SARIF's names: `rules` (globs over `ruleId`), `levels`, `columns` (`utf16CodeUnits`, `unicodeCodePoints`, `bytes`). Only with `output: sarif`. | every result |
| `sandbox` | What the program may reach besides the project's `[sandbox]`: `network` (hosts) and `write` (places). Its lists are added to the project's. | none |

A check adds `files`, globs of the project-relative paths the program is run for; a fix adds `scope`, `safety`, `priority` ([fixer spec](fixer-spec.md)).

## Results

`output` says what the program prints, and so what its result is. There is no role field.

| Result | `output` | Read as |
| --- | --- | --- |
| findings | `lines` | one finding per non-blank stdout line. A leading `path:line[:col]: ` places it. A line that is only the path of a project file, as `gofmt -l`, `rustfmt -l` and `prettier --list-different` print, places it at line 1 of that file with the decision's title as the message, once however often the tool names the file. Any other line attaches to the file the program was run for, or to the project. |
| | `sarif` | one finding per SARIF result, filtered by `sarif`. See [wrapping linters](wrapping-linters.md). |
| | `status` | one finding when the exit code is not in `success`, the tail of stderr or stdout as its message. A program that only passes or fails is a check whose one finding is the failure. |
| changes | `files` | the files the program changed in its scratch copy of the project |
| | `stdout` | stdout is the new text of `{file}`: a filter (`stdin: file`), which needs no copy of the project |

A program that cannot start, outlives its timeout, exits with a code outside `success` or prints more than the output cap is an **error**: a check leaves the analysis incomplete (exit 3), a fix is declined with the tail of stderr as the reason, a verify program withdraws nothing it was not asked to. So is a check under `lines` or `sarif` that exits with a code in `success` other than `0` and reports nothing: a linter that exits 1 with no finding, as `rustfmt` does on a file it cannot parse, did not run clean. `status` is exempt, for its code is its result. An empty set of changes is `unchanged`, with what the program said as the reason; that is how any fixer, a command or an agent, declines.

## Where a program is bound

| Place | Result | Chosen by | Runs |
| --- | --- | --- | --- |
| decision `check: {type: command}` | findings | the decision | once per file (`{file}`), per batch of files (`{files}`), or once over the project |
| decision `fix: {type: command}` | changes | the decision | per finding, in a fix round |
| decision `fix: {type: model}` | changes | the project binds the agent: `[models] agent`, `rules."<id>".agent` | per batch of findings of a file, in a fix round |
| `[fix] verify = [program, …]` | findings (`lines` or `status`) | the project | once per fix run, after the rounds; a failure rolls the newest run of an agent back, and the next, until it passes; their findings are declined |
| `[models.providers.<n>] command` | an answer (a `protocol`), or changes (an agent: no `protocol`) | the project or the user | per classification request, or per fix |

The decision says *what* runs it (`check` and `fix` have the same types: `cel`, `builtin`, `command`, `rpc`, `model`); the project binds *which* model or agent. A bundled decision cannot name the user's agent, model or credentials.

## The command line

No shell: the first element is the program, found on `PATH` or, when it has a `/`, relative to the project root.

- **Placeholders** fill whole arguments, never part of one, so a value cannot grow into an option; a value that starts with `-` is refused. A placeholder the place does not accept is refused when the project loads. One set exists everywhere:

  | Placeholder | Is | Accepted by |
  | --- | --- | --- |
  | `{file}` | the project-relative path of one file | check, fix |
  | `{files}` | as many paths as the argument list allows, over as many runs as it takes | check |
  | `{line}`, `{symbol}` | where the finding is and the symbol it is about | fix |
  | `{decision}` | the id of the decision | check, fix |
  | `{model}` | the model part of the reference | provider |

- **Batching follows the placeholders.** `{files}` runs over many files per run, `{file}` once per file, neither once over the project. A decision whose scope is `file` is run for one file at a time; its results are cached per file text only when the decision is bundled, has no `env`, and the program reads no other file.
- A program the project or a plugin names runs only in a project the user trusts (`lighthouse trust`); a plugin the project names by a `path`, wherever it points, is one, a plugin of the user's plugin directory is not; the trust covers the program and every argument that names a file inside the project, by content. The programs of bundled decisions (`go/formatted`, `rust/formatted`) need none.

## The environment

One allowlist, at every place: `PATH`, `USER`, `LOGNAME`, `LANG`, then `HOME` and `TMPDIR`, then `env`, then `LIGHTHOUSE_DECISION`, `LIGHTHOUSE_OPTIONS` and `LIGHTHOUSE_API_VERSION` where a decision is involved. An agent has a `HOME` and `XDG_*` of its own, empty, so it never sees the user's credentials or settings; every other program keeps the user's.

## Credentials

A key reaches a program in the channel of its reach, with one rule: `${NAME}` is the user's variable `NAME`, with no default and no nesting.

- A program: `env = { NVIDIA_API_KEY = "${NVIDIA_API_KEY}" }`.
- An HTTP provider: `headers = { Authorization = "Bearer ${OPENROUTER_API_KEY}" }`, any scheme (`x-api-key`, `api-key`), plus `body` merged into each request body. At the service a `protocol` names, the header has a default: `openai` sends `Authorization: Bearer ${OPENAI_API_KEY}`, `typesafe` `Bearer ${TYPESAFE_API_KEY}`.
- `${NAME}` expands only in `env` and `headers` values, never in `command`, so a secret is in no argv. The expanded value is never written into a note, a log or a cache key: the key holds the name and a hash of the value, and what a program echoes, on stdout or stderr, is scrubbed to `${NAME}` before it becomes a finding, a note, a notice or an error. A fix that prints the new text of a file (`output: stdout`) is not scrubbed there: its stdout is data, not a message. An unset variable is an error that names the variable.

## Sandbox and the network

Every program runs in the [OS sandbox](architecture.md#sandbox). It reads everything but secrets, writes only its own places, and reaches only the hosts listed. The project's `[sandbox] network` and `write` apply to every program; a program's own `sandbox` lists are added, so a provider's hosts do not widen a check's. A program is **remote** when the hosts it may reach, the project's `[sandbox] network` and its own `sandbox.network` together, are not empty, or its provider's `url` is not loopback: what leaves the machine is what reaches a host. A `url` is loopback when its host is exactly `localhost` or an IP address in `127.0.0.0/8` or `::1`; `127.attacker.example` is not. A remote provider is the one a budget and routing apply to. On macOS, Seatbelt blocks FSEvents, so the file watchers inside agents fail harmlessly. Known limit on macOS: there is no PID namespace, so a program that starts a daemon with `setsid` leaves the process group, and the timeout does not kill it; on Linux the namespace takes it down with the program.

## One path for a change

A command fix and an agent change files by the same path.

1. **A scratch copy** of the project, secrets excluded, with the fixed texts of the run in place of their files, as a repository with one commit of that state: a tool finds its root, `git diff` works, and an agent that refuses to run outside a repository runs. The copy holds the files the project's own ignore files let through, not those of the user's global excludes, so it does not depend on one machine; git runs in it with every `GIT_*` variable cleared and `-C` the copy, so an inherited `GIT_DIR` never reaches the user's repository. Clones are copy-on-write where the file system has them. A filter (`stdin: file`, `output: stdout`) needs no copy.
2. **One collection** of what changed in it against the texts it started from.
3. **One scope check**, `file` (the finding's file; the default of a command fix), `symbol`, `references` or `files` ([fixer spec](fixer-spec.md)). A change outside it rejects the whole attempt.
4. **One outcome mapping** to the classes `unchanged`, `out-of-scope`, `failed`, `timeout`, `introduced`, `conflict`.

Then the orchestrator applies the change in memory, re-checks it, adds the safe fixes of what it introduced, and writes only what the checks accept ([architecture](architecture.md#fixing)).

Programs at a decision and at verify are deterministic by contract. Agents are not: they go through routes, budget, failover and the edit cache ([models](models.md)).
