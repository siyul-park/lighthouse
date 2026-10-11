# Models

A model is a plugin kind beside language providers: a process (`rpc`) that answers
`lighthouse/classify` (and `lighthouse/train` when it learns), or any program (`command`).
Lighthouse defines no API of its own. The wire types are one serde module of `lighthouse-protocol`
with a JSON Schema (`protocol/schema/lighthouse-protocol-0.1.json`), and each is a published
body used verbatim: TypeSafe System One for classification (`SystemOneRequest`
`{state, model, questions}` to `SystemOneResponse` `{model, answers, usage}`), OpenAI's
fine-tuning job for training, OpenAI's model list for the manifest. The provider's `options`
travel beside the System One body, never inside it. Errors are JSON-RPC errors whose `data` is the
reused API's own error body. No HTTP server is built: a server that speaks these bodies could be
declared as a provider later.

## Run time

A rule hands the subjects it suspects (`review`) to `Ctx::models`. The rule only queues the
questions; the engine flushes them after the rules of a file (`engine/batch.rs`), one request per
subject with up to 32 questions (one per decision), so that a subject is sent once however many
decisions ask about it. The engine resolves the decision's bindings (Revision, rule, project
default), asks each model in turn for the subjects the previous one left open, applies the budget
and the report scope to models that cost, keeps every answer in the result cache (the raw `noul`)
under the model's reference and version, and returns for each subject a decision or the reason it
stays open. The host owns the band: `noul` at or above `UPPER` (0.9) fails the subject with
confidence `max(p, 1 - p)`; anything else is left to review. Open subjects come back as
diagnostics marked `review`; the engine reports them apart from findings (`Outcome::reviews`), and
they never change the exit code. `Outcome::subjects` keeps what was put to the models when a
command (`eval`, `learn train`) asks for it.

## `model-http`

`plugins/model-http` serves the standard HTTP APIs. Each published API is one module of free
functions, one per operation used, named by the API's operation in snake_case, with the signature
`fn(client: &dyn Client, endpoint: &Endpoint, request: &Req) -> Result<Resp, HandlerError>`:

| Module | Operation | Types |
| --- | --- | --- |
| `openai` | `create_chat_completion` (`POST /chat/completions`) | `async-openai` (types only, OpenAPI-generated) |
| `typesafe` | `systemone` (`POST /v1/systemone`) | the protocol's `SystemOneRequest` / `SystemOneResponse` |
| `chat` | `systemone` over `openai::create_chat_completion` | the adaptation: one rendered prompt per question, `{noul}` by `json_schema` |

`http.rs` holds the one I/O seam, `trait Client { fn post(url, key, body) }`, and `Http` (bearer
key, retries on 429, 503 and 529 honouring `Retry-After`, the error body as the JSON-RPC `data`).
Tests swap the client, never a module. `dispatch.rs` is the table from protocol method to
functions, the method's own API first: `lighthouse/classify` goes to `typesafe::systemone`, then
`chat::systemone` for `openai`; the first whose `spec` the provider declares serves the call.

An official SDK in another language needs no code: a `command` provider with `stdin = "request"`
runs a script that reads System One's body and prints its answer, for example:

```python
# classify.py: argv = ["python3", "classify.py"], stdin = "request", pointer unset
import json, sys
from openai import OpenAI

request = json.load(sys.stdin)
client = OpenAI()
answers = {}
for id, question in request["questions"].items():
    reply = client.chat.completions.create(
        model=request["model"],
        messages=[
            {"role": "system", "content": question["instructions"]["prompt"]},
            {"role": "user", "content": request["state"]["text"]},
        ],
        response_format={"type": "json_schema", "json_schema": {
            "name": "classification", "strict": True,
            "schema": {"type": "object", "properties": {"noul": {"type": "number"}},
                       "required": ["noul"], "additionalProperties": False}}},
    )
    answers[id] = {"type": "noul", "noul": json.loads(reply.choices[0].message.content)["noul"]}
print(json.dumps({"model": request["model"], "answers": answers}))
```

Anthropic's and TypeSafe's Python SDKs fit the same way: read the body, call the SDK, print
`{answers: {<decision id>: {noul}}}`.

## Labels, datasets, `eval` and `learn`

Session owns the labels (`labels`), the metrics (`metrics`), `eval` and `learn`; the CLI only
parses and prints. Both `eval` and `learn train` collect labels the same way: a subject has one
label, the strongest source winning (person, a reviewed table, a coding agent, a model; the latest
within one source). A judgment's source comes from its PROV attribution: a person, or a software
agent by the role it had (`hadRole`, agent or model). The decision's examples are subjects too,
run through the project's own configuration. `eval` grades only on weight 1.0 labels.

`learn train` writes the training lines (OpenAI's chat fine-tuning format, canonical JSON per
line, with `weight`, `key` and `attribution` beside the messages) to
`.lighthouse/datasets/sha256/<hex>.jsonl`, named by the digest of the file, and hands the job
request, with `training_file = "sha256:<hex>"` and the dataset directory in `metadata`, to the
model. `--from-model` distils a teacher model's confident answers (fail and pass) into labels of
weight 0.5.
