# Models

A model is a plugin kind beside language providers: a process (`rpc`) that answers
`lighthouse/classify` (and `lighthouse/train` when it learns), or any program (`command`).
Lighthouse defines no API of its own. The wire types are in `lighthouse-protocol`, with a JSON
Schema (`protocol/schema/lighthouse-protocol-0.3.json`). Each request is the published API body
plus provider `options`, omitted when empty. Classification uses TypeSafe System One
(`SystemOneRequest` `{state, model, questions, options?}` to `SystemOneResponse`
`{model, answers, usage}`). HTTP providers remove `options` before posting the body. Training and
model listing use OpenAI's `CreateFineTuningJobRequest` and `ListModelResponse`; each offered model
uses one `Model` type. Errors are JSON-RPC errors whose `data` is the
reused API's own error body. No HTTP server is built: a server that speaks these bodies could be
declared as a provider later.

## Run time

A rule evaluates its `abstain` route only when the run asks its models (`Models::asks`): a model is
bound to its decision through a Revision, `rules."<id>".model` or `[models] classification`, or
the run passes `--all-models` or collects subjects. Otherwise the route is skipped, nothing is
reported for it, and the rule's findings stay cacheable (a rule that asks is never stored). The
`where` part is unaffected. A rule hands the subjects it suspects (`abstain`) to `Context::models`. The rule only queues the
questions; the engine flushes them after the rules of a file (`engine/batch.rs`), one request per
subject with up to 32 questions (one per decision), so that a subject is sent once however many
decisions ask about it. The engine resolves the decision's bindings (Revision, rule, project
default), asks each model in turn for the subjects the previous one left open, applies the budget
and the report scope to models that cost, keeps every answer in the result cache (the raw `noul`)
under the model's reference and version, and returns for each subject a decision or the reason it
stays open. The host owns the band: `noul` at or above `FAIL_AT` (0.9) fails the subject with
confidence `max(p, 1 - p)`; anything else is left to review. Open subjects come back as
diagnostics marked `review`; the engine reports them apart from findings (`Run::reviews`), and
they never change the exit code. `Run::questions` keeps what was put to the models when a
command (`decision eval`, `model train`) asks for it.

## Classification

The probability that a subject fails comes from the model's own distribution: the logprobs of the
answer token of a chat model, the score of a classifier, or System One's `noul`. A number a model
writes in its reply (a verbalized probability) is never one, and neither is the verdict of an
agent CLI such as `claude -p` or `codex exec`: it has no distribution to read. Agent CLIs are
`chat` providers, a `command` without a protocol (below).

## Chat and fixes

`[models] chat` binds the model that writes the fixes no rule derives: one reference, or a route
list that fails over on error like a classification route (layers are for classification only).
It serves the task `chat` (`lighthouse/chat`, whose body is OpenAI's Chat Completions; HTTP twin
`POST /v1/chat/completions`). A provider serves a task by its handshake (plugin) or its protocol:
`openai` (HTTP or `command`) and a `command` without a protocol serve `chat`; `typesafe` serves
classification only; `model-local` serves neither. Binding a provider to a task it does not serve is
a configuration error naming both.

A decision with a check and no fix of its own then has a fix of type `model`; a decision may also
say `fix: {type: model, prompt: "..."}` to add an instruction. A decision whose fix is `ops` or
`command` never uses the model. Without a `chat` binding nothing changes.

```toml
[models]
chat = "openrouter/qwen/qwen3-coder:free"
[approval]
fix = "preview"            # default; "verified" writes a verified model fix
[fix]
verify = [["cargo", "test", "-q"]]   # must pass over the model fixes of a run
timeout = "10m"            # for each of them
```

- **What the model sees.** One extent per finding: the symbol the finding is about, else the
  innermost symbol around it, else the whole file when it has fewer than 400 lines (otherwise the
  finding is declined, "too large for a model fix without a symbol"). The prompt is fixed text plus
  the decision (requirement, context, `prompt`), the finding (message, evidence, place in the
  extent), the extent verbatim and up to three of the decision's invalid examples that have
  `fixed`. The request is `temperature: 0` with a strict `json_schema` response format
  `{replacement}` (omitted for plain-text commands, whose answer is searched for the first JSON
  object). The answer is one `Replace` of the extent, `suggested` and never `safe`.
- **What keeps it.** The orchestrator applies the replacement in memory, formats, re-checks, and
  keeps it only if the finding's fingerprint is gone (a new finding of the same rule in the file
  also counts as "did not resolve the finding"), no new `warn` or `error` appeared in the file
  ("introduced <decision> at <line>"), and the file is still fully analyzed. Otherwise the file is
  rolled back and the finding declined with the reason. Deterministic fixes keep their own rule.
- **`[fix] verify`.** Commands (argument lists, no shell) run once per run, after the fixes, over a
  scratch copy of the project (what version control keeps, with the fixed texts in place), when the
  model fixes would be written. A failure withdraws every model fix of the run to a preview and
  puts the command and the tail of its output in the messages. They need a trusted project and are
  part of what `lighthouse trust` lists.
- **Approval.** With `approval.fix = "preview"` a verified fix is not written: the report carries it
  as a preview with its diff (`previews` in the `fix` JSON, `preview ...` lines in `check --fix`),
  awaiting a hand. With `"verified"` it is written like any applied fix. `--unsafe-fixes` also
  writes verified model fixes whatever the approval: the approval is the project's standing
  permission for model fixes only, the flag is the person's permission for all suggested fixes in one
  run. `--dry-run` writes nothing either way.
- **Accounting.** Per decision: `asked`, `answered`, `verified`, `applied`, `previewed` and `declined`
  by class (`no-extent`, `unanswered`, `out-of-scope`, `unparsed`, `unchanged`, `unresolved`,
  `introduced`, `incomplete`, `rolled-back`), in `check --fix` and in the `fix` JSON (`models`),
  with `usage` (requests and cost; for `edit` also `runs` and `seconds`).
- **Cost, trust, cache.** Requests count against `[models] budget` at the provider's `cost`; remote
  providers need the opt-in classification does; `command` providers need `lighthouse trust`; the
  answers are kept in the result cache under the model, its version and the digest of the request
  (`--no-cache` skips it, `--no-models` asks no model).

Recipes (the keys are the provider axes of [rule-pipeline.md](rule-pipeline.md#models)):

```toml
[models.providers.openrouter]     # a free model, key from the environment
protocol = "openai"
baseUrl = "https://openrouter.ai/api/v1"
keyEnv = "OPENROUTER_API_KEY"
cost = 1

[models.providers.ollama]         # Ollama, LM Studio (:1234) and llama-server (:8080) alike
protocol = "openai"
baseUrl = "http://localhost:11434/v1"
cost = 0

[models.providers.claude]         # a plain-text command: stdin is the conversation, stdout the answer
command = ["claude", "-p", "--tools", "", "--no-session-persistence"]
# reference: claude/any; `--tools ""` leaves it no tools, and no flag skips permissions
```

## Edit and fixes by an agent

`[models] edit` binds an agent that fixes by changing files: one reference, or a route list that
fails over on error. Only a `command` without a `protocol` serves it, in addition to `chat`:
running the same program in a scratch copy of the project is what makes it an edit. HTTP and
plugin providers do not serve `edit`, and binding one is a configuration error ("provider `x` does
not serve `edit`: only a command without a protocol does"). When `edit` is bound, fixes by a model
use it instead of `chat`; one binding is used per run. Without it nothing changes.

```toml
[models]
edit = "claude/sonnet"
[models.providers.claude]
command = ["claude", "-p", "--output-format", "text", "--permission-mode", "acceptEdits",
           "--allowedTools", "Read,Edit,Write,Glob,Grep", "--model", "{model}"]
timeout = "10m"            # the default for edit
[approval]
fix = "preview"
```

```toml
[models]
edit = "codex/any"
[models.providers.codex]   # the model is codex's own configuration
command = ["codex", "exec", "--sandbox", "workspace-write", "--skip-git-repo-check", "-"]
timeout = "10m"
```

Both recipes use the narrowest permission the tool offers for editing files in its working
directory: `claude -p` (prompt on stdin) with `--permission-mode acceptEdits` and only the file
tools in `--allowedTools`; `codex exec` (prompt on stdin through `-`) with `--sandbox
workspace-write`, and `--skip-git-repo-check` because the scratch copy has no repository. Never use
a flag that skips permissions or the sandbox (`--dangerously-skip-permissions`, `--yolo`,
`--full-auto`, `danger-full-access`); a test checks that no recipe here does. The flags were checked
against the Claude Code headless docs (<https://code.claude.com/docs/en/headless> and `claude
--help`) and the Codex non-interactive docs (<https://learn.chatgpt.com/docs/non-interactive-mode>,
formerly `developers.openai.com/codex/noninteractive`); `codex exec --help` could not be run on the
machine that wrote this, so `--skip-git-repo-check` and `-` rest on those docs alone.

- **Workspace.** A scratch copy of the project (what version control keeps, with the run's
  fixed texts in place; the same copy `[fix] verify` uses). The command runs with it as its working
  directory and the prompt as plain text on stdin, under the process contract (no shell, own process
  group, `timeout`, output cap, environment of `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `TMPDIR`
  plus `env` and `keyEnv`, project trust). The copy is deleted when the run ends, on every path.
  stdout is kept for the notes only (2 KB).
- **One run per file.** Every model-fixable finding of the file in the report scope, up to 20, by
  line; files run one after another. The prompt names, per finding, the decision (requirement,
  context, `prompt`), the message, the evidence and `path:line:col`, with up to two before and after
  examples per decision, and asks for one line per finding in reply; the reply is a note, never a
  verdict.
- **What it may change.** The text of that file, nothing else. A change to another file, to a file
  outside the report scope or one Lighthouse does not index, a file created or deleted, or a
  non-text file rejects the whole attempt (class `out-of-scope`, the reason names the first
  path). A non-zero exit, a timeout or a signal is a model error: the route fails over, else the
  findings are declined `unanswered`.
- **What keeps it.** The file becomes one `Replace` of the whole file per finding (`suggested`,
  "model edit by `<provider>/<model>`"), verified as every fix by a model, per file: kept only if
  at least one targeted finding is gone, nothing new of `warn` or `error` is in the file, and the
  file is fully analyzed. Targeted findings still there are declined `unresolved`; the rest count
  as `verified`. Then `[fix] verify` and `[approval]` apply as for `chat`.
- **Cache.** The key is the model and its version, the digest of the prompt and the digests of the
  files in the report scope (all an agent may read); the value is the changed texts. A hit runs no
  process and spends no budget.
- **Safety.** Trusted projects only. With a `keyEnv` the provider is remote; otherwise it is a
  local process, and the agent's own configuration decides where code is sent. Lighthouse writes
  nothing outside the scratch copy; the files are written by the fix run after verification, as for
  any fix.

## Providers

A provider has two axes: the `protocol` it speaks and its reach, exactly one of `baseUrl` (HTTP,
served by `model-http`), `command` (a stdio process speaking `typesafe`) and `plugin` (an rpc
plugin, whose handshake says what it serves). `protocol = "openai"` or `"typesafe"` alone is HTTP
at that protocol's own service. Recipes are in [rule-pipeline.md](rule-pipeline.md#models).

## `model-http`

`plugins/model-http` serves the standard HTTP protocols. Each published protocol is one module of
free functions, one per operation used, named by the protocol's operation in snake_case, with the
signature
`fn(client: &dyn Client, endpoint: &Endpoint, request: &Req) -> Result<Resp, ErrorObject>`:

| Module | Operation | Types |
| --- | --- | --- |
| `openai` | `create_chat_completion` (`POST /chat/completions`) | `async-openai` (types only, OpenAPI-generated) |
| `typesafe` | `systemone` (`POST /v1/systemone`) | the protocol's `SystemOneRequest` / `SystemOneResponse`; `options` stripped |
| `chat` | `systemone` over `openai::create_chat_completion` | the adaptation: one prompt per question, P(fail) from the logprobs of `true` and `false` |

`http.rs` holds the one I/O seam, `trait Client { fn post(url, key, body) }`, and `Http` (bearer
key, retries on 429, 503 and 529 honouring `Retry-After`, the error body as the JSON-RPC `data`).
Tests swap the client, never a module. `lighthouse/classify` goes to `typesafe::systemone`, then
`chat::systemone` for `openai`; the first whose `protocol` the provider declares serves the call.
A response without logprobs is an error, since the model cannot serve classification.

An official SDK in another language needs no code: a `command` provider with `protocol =
"typesafe"` runs a script that reads System One's body and prints its response, for example:

```python
# classify.py: protocol = "typesafe", command = ["python3", "classify.py"]
import json, math, sys
from openai import OpenAI

request = json.load(sys.stdin)
client = OpenAI()
answers = {}
for id, question in request["questions"].items():
    reply = client.chat.completions.create(
        model=request["model"],
        messages=[
            {"role": "system", "content": question["instructions"]["prompt"]
                + " Answer with one word: true if the subject violates it, false if it complies."},
            {"role": "user", "content": request["state"]["text"]},
        ],
        logprobs=True, top_logprobs=20, max_tokens=1, temperature=0,
    )
    top = reply.choices[0].logprobs.content[0].top_logprobs
    mass = lambda label: sum(math.exp(t.logprob) for t in top if t.token.strip().lower() == label)
    p_true, p_false = mass("true"), mass("false")
    if p_true + p_false > 0:
        answers[id] = {"type": "noul", "noul": p_true / (p_true + p_false)}
print(json.dumps({"model": request["model"], "answers": answers}))
```

TypeSafe's SDK fits the same way: read the body, call the SDK, print
`{answers: {<decision id>: {noul}}}`. An answer left out is an abstention.

## Labels, datasets, `decision eval` and `model train`

Session owns the labels (`labels`), the metrics (`metrics`), `decision eval` and `model train`; the CLI only
parses and prints. Both `decision eval` and `model train` collect labels the same way: a subject has one
label, the strongest source winning (person, a reviewed table, a coding agent, a model; the latest
within one source). A judgment's source comes from its PROV attribution: a person, or a software
agent by the role it had (`hadRole`, agent or model). The decision's examples are subjects too,
run through the project's own configuration. `eval` grades only on weight 1.0 labels.

`model train` writes the training lines (OpenAI's chat fine-tuning format, canonical JSON per
line, with `weight`, `key` and `attribution` beside the messages) to
`.lighthouse/datasets/sha256/<hex>.jsonl`, named by the digest of the file, and hands the job
request, with `training_file = "sha256:<hex>"` and the dataset directory in `metadata`, to the
model. A successful job identifies its artifact as `learned@sha256:<hex>` or
`general@sha256:<hex>`. `--from-model` distils a teacher model's confident answers (fail and pass) into labels of
weight 0.5.
