# Models

A model is a plugin kind beside language providers: a process (`rpc`) that answers
`lighthouse/classify` (and `lighthouse/train` when it learns), or any program (`command`).
Lighthouse defines no API of its own. The wire types are in `lighthouse-protocol`, with a JSON
Schema (`protocol/schema/lighthouse-protocol-0.2.json`). Each request is the published API body
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
agent CLI such as `claude -p` or `codex exec`: it has no distribution to read. Agent CLIs become
`chat` and `edit` providers, a `command` without a protocol, when fixes by models arrive.

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
