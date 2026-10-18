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
bound to its decision through a Revision, `rules."<id>".classification` or `[models] classification`, or
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
`edit` providers, a `command` without a protocol (below).

## Fixes by a model

Lighthouse is the System 1 harness: deterministic checks, classifiers, the verifier and briefs.
Generating or changing files is System 2, an external agent's work, so there is no chat task and no
prompt-and-parse fixer. Chat Completions serve classification only (see `model-http` below). A model
fixes a finding in one way: `[models] edit` binds an agent that changes files in a scratch copy of
the project, and the project permits it decision by decision. Nothing else enables a model fix;
binding an agent enables nothing by itself.

```toml
[models]
edit = "claude/sonnet"                             # the agent, below
[fix]
decisions = ["design/exported-doc", "testing/*"]   # ids or globs; default none
scope = "files"                                    # widest change: symbol | files; default files
verify = [["cargo", "test", "-q"]]                 # must pass over the fixes of a run
timeout = "10m"                                    # for each of them
[approval]
fix = "ask"                                        # default; "allow" writes a verified model fix
```

- **Who gets a model fix.** A finding does if its decision matches `[fix] decisions`, has a check
  and no fix of its own (`ops`, `command`), and `[models] edit` is bound. An entry is an id or a glob
  (`*` within one segment of an id, matched against the whole id); one that matches no active
  decision is a warning naming it. A listed decision without a `fix` block has `fix: {type: model}`;
  a decision may say `fix: {type: model, prompt: "...", scope: symbol | files}` to add an
  instruction and a scope. A decision that declares none imposes no limit of its own, so its scope is
  `[fix] scope`; one that declares a scope is narrowed to `[fix] scope`, never widened by it.
- **Edit or review, no router.** Where a finding gets a model fix it is `edit`; with no `edit` bound
  it stays a review task that carries the brief (MCP `review_show` returns it as `brief`, `review
  show` prints it under `brief:`) for whoever drives the session, and Lighthouse re-checks when they
  are done. There are no retries inside Lighthouse. The brief is attached to the review task of every
  listed decision.
- **The brief.** One builder writes it, for the `edit` prompt and for the review task: the system part
  (change only what the findings need, within the scope of each: `symbol` is the symbol of the
  finding, `files` any file the fix needs), for each finding the decision (requirement, context,
  `prompt`), message, evidence, `path:line:col` and scope, up to two before and after examples per
  decision, how the result is checked, and a closing line asking for `fixed` or `left` per finding.
  The reply is a note, never a verdict.
- **Scope, checked by the verifier.** `symbol`: only the extent of each finding of the batch in its
  file (the symbol the finding is about, else the innermost around it, else the whole file when it has
  fewer than 400 lines, otherwise the finding is declined `no-extent`); a change outside the union of
  the extents is declined `out-of-scope`, naming the first line. `files`: any indexed text file in
  the run's scope; each changed file is one whole-file `Replace`, and the files are verified together
  with every file that calls or refers to a symbol whose signature changed (a symbol with no signature
  fact counts as changed); the change passes or is rolled back whole.
- **Safe fixes first.** Before an edit is judged, the safe fixes of the rules (`safety: safe`, the
  rounds of `check --fix`, whatever the run selected) are applied to the files it changed, to a
  fixpoint, so an order or layout rule such as `design/declaration-groups` does not condemn code an
  agent placed well enough. The scope is checked on the agent's own diff, not on these edits, and the
  brief lists the decisions with a safe fix for the language of the file ("the project's tools fix
  these after you; follow them when placing code"). The preview and the written text are the result
  after them.
- **What keeps it.** The orchestrator applies the files in memory, formats, re-checks, and keeps the
  edit only if at least one targeted finding is gone, no decision has more findings of `warn` or `error` in
  the changed and referencing files than before (counted by decision and file, so a renamed symbol's
  old findings are not new; "introduced <decision> at <line>"), and they are fully analyzed.
  Targeted findings still there are declined `unresolved`; the rest count as `verified`. Otherwise the
  files are rolled back and the findings declined with the reason.
- **Runs in sequence.** One run for each file and scope of the findings (every model-fixable finding
  of the file with that scope in the report scope, up to 20, by line), one after another by path,
  the narrower scope first: a chain. A `symbol` decision and a `files` decision in one file are two
  runs. Each run's scratch copy holds the project plus every edit already accepted in the pass, so
  whole-file results never overwrite each other. An edit is accepted only if each file it changed
  still has the digest it found (compare and swap); otherwise it is declined `conflict` and never
  merged. The cache key includes those digests. Each run is applied, re-checked and judged by
  itself, over what the runs before it left; one made over a text an earlier run did not keep is
  declined `conflict`. Then `[fix] verify` runs; on failure the newest run is undone (its files go
  back to the text it started from), then the one before it, until the commands pass, and the
  findings of those undone are declined `rolled-back` with the reason.
- **`[fix] verify`.** Commands (argument lists, no shell) run once per run, after the fixes, over a
  scratch copy of the project (what version control keeps, with the fixed texts in place), when the
  fixes would be written. They need a trusted project and are part of what `lighthouse trust` lists.
- **Approval.** With `approval.fix = "ask"` a verified fix is not written: the report carries it as a
  preview with its diff (`previews` in the `fix` JSON, `preview ...` lines in `check --fix`), awaiting
  a hand; applying it is `check --fix --unsafe-fixes --fingerprint <fp>`, which serves the edit from
  the cache (the model is not asked again) and re-verifies it, `conflict` if the files changed. With
  `"allow"` it is written like any applied fix. `--unsafe-fixes` also writes verified model fixes
  whatever the approval: the same permission every unsafe fix of every provider has. `--dry-run`
  writes nothing either way.
- **Accounting.** Per decision: `asked`, `answered`, `verified`, `applied`, `previewed` and `declined`
  by class (`no-extent`, `unanswered`, `out-of-scope`, `conflict`, `unchanged`, `unresolved`,
  `introduced`, `incomplete`, `rolled-back`), in `check --fix` and in the `fix` JSON (`models`), with
  `usage` (requests and cost, and `runs` and `seconds`).
- **Cost, trust, cache.** Requests count against `[models] budget` at the provider's `cost`; remote
  providers need the opt-in classification does; `command` providers need `lighthouse trust`; the
  edits are kept in the result cache under the model, its version, the digest of the brief and the
  digests of the files in the report scope as they stand (`--no-cache` skips it, `--no-models` asks
  no model).

## The agent that edits

Only a `command` without a `protocol` serves `edit`: running the program in a scratch copy of the
project, with the brief as plain text on stdin, is what makes it an edit. HTTP and plugin providers do
not, and binding one is a configuration error ("provider `x` does not serve `edit`: only a command
without a protocol does"); a `command` with `protocol = "openai"` is no provider at all ("a `command`
speaks `typesafe` (classification) or plain text (edit)"). `[models] edit` is one reference or a
route list that fails over on error; `rules."<id>".edit` binds another one for that decision, in the
project file or in a user or local file ([architecture.md](architecture.md#configuration-layers)).
The findings of a file are fixed by one run for each scope and agent, one run after another, each
over the texts the runs before it were accepted with. The agent CLIs `claude -p`, `codex exec` and opencode are `edit`
providers; none of them classifies, since they have no distribution to read.

```toml
[models]
edit = "claude/sonnet"
[models.providers.claude]
command = ["claude", "-p", "--output-format", "text", "--permission-mode", "acceptEdits",
           "--allowedTools", "Read,Edit,Write,Glob,Grep", "--model", "{model}"]
timeout = "10m"            # the default for edit
keyEnv = "ANTHROPIC_API_KEY"      # its login is not in the home of its own
network = ["api.anthropic.com"]   # the hosts the agent may reach; none by default
```

```toml
[models]
edit = "codex/any"
[models.providers.codex]   # the model is codex's own configuration
command = ["codex", "exec", "--sandbox", "workspace-write", "--skip-git-repo-check", "-"]
timeout = "10m"
keyEnv = "OPENAI_API_KEY"
network = ["api.openai.com"]
```

The sandbox of `codex` runs inside Lighthouse's; whether the two nest on every OS is not verified.

```toml
[models]
edit = "oc/nim/z-ai/glm-5.3"        # provider oc; opencode's own model is nim/z-ai/glm-5.3
[models.providers.oc]
command = ["opencode", "run", "--pure", "--model", "{model}"]   # the prompt arrives on stdin
keyEnv = "NVIDIA_API_KEY"           # passed through; opencode reads it as {env:NVIDIA_API_KEY}
network = ["integrate.api.nvidia.com"]
timeout = "10m"
[models.providers.oc.env]
OPENCODE_DISABLE_PROJECT_CONFIG = "1"   # only the configuration below applies
OPENCODE_CONFIG_CONTENT = '''
{"permission": {"*": "deny", "read": "allow", "list": "allow", "glob": "allow", "grep": "allow", "edit": "allow"},
 "provider": {"nim": {"npm": "@ai-sdk/openai-compatible",
   "options": {"baseURL": "https://integrate.api.nvidia.com/v1", "apiKey": "{env:NVIDIA_API_KEY}"},
   "models": {"z-ai/glm-5.3": {}}}}}
'''
```

The opencode recipe allows only the file tools through its `permission` table, in the configuration
it is given instead of the project's, and `--pure` runs it without external plugins. The provider
block is any OpenAI-compatible service; the key stays in the environment. Never add `--auto`: it
approves every permission request.

The `claude` and `codex` recipes use the narrowest permission the tool offers for editing files in its working
directory: `claude -p` (prompt on stdin) with `--permission-mode acceptEdits` and only the file
tools in `--allowedTools`; `codex exec` (prompt on stdin through `-`) with `--sandbox
workspace-write`, and `--skip-git-repo-check` because the scratch copy has no repository. Never use
a flag that skips permissions or the sandbox (`--dangerously-skip-permissions`, `--yolo`,
`--full-auto`, `danger-full-access`); a test checks that no recipe here does. The flags were checked
against the Claude Code headless docs (<https://code.claude.com/docs/en/headless> and `claude
--help`) and the Codex non-interactive docs (<https://learn.chatgpt.com/docs/non-interactive-mode>,
formerly `developers.openai.com/codex/noninteractive`); `codex exec --help` could not be run on the
machine that wrote this, so `--skip-git-repo-check` and `-` rest on those docs alone.

- **Workspace.** A scratch copy of the project (what version control keeps, with the run's accepted
  texts in place; the same copy `[fix] verify` uses). The command runs with it as its working
  directory under the process contract (no shell, own process group, `timeout`, output cap,
  environment of `PATH`, `USER`, `LOGNAME`, `LANG` plus `env` and `keyEnv`, project trust) and in
  the [sandbox](architecture.md#sandbox): it writes only in the copy and in a `TMPDIR` and a `HOME`
  (with `XDG_*`) of its own, which `env` may override, it reads everything but the sandbox's list of
  secrets (the user's credential files and the project's `.env` files, which the copy leaves out),
  and it reaches only the hosts the provider lists in `network`. The copy is deleted when the run ends, on every path. stdout is kept for the notes only
  (2 KB).
- **What it may change.** Under the scope, the indexed text files of the report scope. A change to a
  file outside the report scope or one Lighthouse does not index, a file created or deleted, or a
  non-text file rejects the whole attempt (class `out-of-scope`, the reason names the first path). A
  non-zero exit, a timeout or a signal is a model error: the route fails over, else the findings are
  declined `unanswered`.
- **Safety.** Trusted projects only. With a `keyEnv` the provider is remote; otherwise it is a local
  process, and the agent's own configuration decides where code is sent. Lighthouse writes nothing
  outside the scratch copy; the files are written by the fix run after verification, as for any fix.
  An agent that logs in through a file in the home of the user does not find it in the home of its own: give it a `keyEnv`.

## Providers

A provider has two axes: the `protocol` it speaks and its reach, exactly one of `baseUrl` (HTTP,
served by `model-http`), `command` (a stdio process speaking `typesafe`) and `plugin` (an rpc
plugin, whose handshake says what it serves). `protocol = "openai"` or `"typesafe"` alone is HTTP
at that protocol's own service. Recipes are in [rule-pipeline.md](rule-pipeline.md#models).

```toml
[models.providers.openrouter]     # a hosted model with logprobs, key from the environment
protocol = "openai"
baseUrl = "https://openrouter.ai/api/v1"
keyEnv = "OPENROUTER_API_KEY"
cost = 1
# timeout = "10m"                 # per request, default 120s; reasoning models may need it

[models.providers.ollama]         # Ollama, LM Studio (:1234) and llama-server (:8080) alike
protocol = "openai"
baseUrl = "http://localhost:11434/v1"
cost = 0
```

An `openai` HTTP provider serves classification only (Chat Completions with logprobs, never fixes).


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
