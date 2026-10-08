//! The tools the server offers: name, description and the JSON Schema of the
//! arguments. `decision_similar` and `decision_proposals` are reserved for
//! later phases and deliberately absent.

use rmcp::model::{JsonObject, Tool};
use serde_json::{Value, json};

const FORMATS: &str = "ids are fully qualified, `<pack>/<name>`";

pub fn all() -> Vec<Tool> {
    [
        tool(
            "check",
            "Check the project and report findings as agent records (rule, location, requirement, evidence, expected structure, fingerprint) with a summary. Analysis always covers the whole project; the arguments only narrow what is reported. `status` is `clean` only when nothing is reported AND the analysis was complete: `incomplete` is never a pass.",
            json!({
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Report only under these paths (relative to the server's working directory)." },
                "changed": { "type": "boolean", "description": "Report only files changed in the working tree against HEAD (untracked included)." },
                "diff": { "type": "string", "description": "Report only files changed since the merge base with this git ref." },
                "rules": { "type": "array", "items": { "type": "string" }, "description": format!("Run only these rules; {FORMATS}.") },
                "limit": { "type": "integer", "minimum": 1, "description": "Most findings to return, errors first (default 25)." }
            }),
            &[],
        ),
        tool(
            "fix",
            "Fix findings with the fixers the rules' decisions name (`fixable: safe|suggested` in `decision_list` and `explain`). Select what to fix with `fingerprints` (findings from `check`, prefixes allowed), `paths` or `rules`; at least one is required. A fix is proposed as edits over the code model, applied, formatted, re-checked and rolled back for any file that gains an error or stops being analyzed, and repeated until nothing is left (at most 5 rounds). Only safe fixes of mechanical rules are applied unless `unsafeFixes` is set. Use `dryRun` first to see the unified `diff` without changing a file. Returns the `diff`, the `applied` fixes and the `declined` findings with the reason each was left alone; recheck with `check` afterwards. Findings a verdict suppresses are never fixed.",
            json!({
                "fingerprints": { "type": "array", "items": { "type": "string" }, "description": "Fix these findings; a fingerprint or an unambiguous prefix each." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Fix findings under these paths (relative to the server's working directory)." },
                "rules": { "type": "array", "items": { "type": "string" }, "description": format!("Fix findings of these rules; {FORMATS}.") },
                "dryRun": { "type": "boolean", "description": "Return the diff and leave every file as it was." },
                "unsafeFixes": { "type": "boolean", "description": "Also apply suggested fixes, which are judgment calls: review the diff." }
            }),
            &[],
        ),
        tool(
            "explain",
            "Explain a decision or rule: intent, requirement, examples, exceptions and status, as Markdown.",
            json!({ "id": { "type": "string", "description": "A decision or rule id such as `design/exported-doc`." } }),
            &["id"],
        ),
        tool(
            "decision_list",
            "List the decisions with severity, title and whether this project's configuration enables them.",
            json!({ "all": { "type": "boolean", "description": "Include every catalog decision, checked or not." } }),
            &[],
        ),
        tool(
            "review_tasks",
            "List remembered findings that ask for a verdict (by default the open findings of heuristic and judgment decisions, whatever their severity), each with the fingerprint and `last_seen` that `review_resolve` needs.",
            json!({
                "status": { "type": "string", "enum": ["open", "suppressed", "narrowing", "inactive", "resolved", "all"], "description": "Default `open`." },
                "rule": { "type": "string", "description": "Only this rule." },
                "tier": { "type": "string", "enum": ["review", "all"], "description": "Default `review`: findings that ask for a verdict because their decision is a heuristic or a judgment. `all` lists every remembered finding." },
                "limit": { "type": "integer", "minimum": 1, "description": "Default 50." }
            }),
            &[],
        ),
        tool(
            "review_resolve",
            "Record a verdict on a finding, as an agent review. confirmed (reason fixed or accepted-debt) means the finding is right; rejected (false-positive, intentional-exception, scope-too-broad, project-allowed, not-worth-fixing) needs a reason and keeps a non-mechanical finding out of later reports while its rule and evidence stay as they are; deferred takes no reason. Mechanical findings are recorded but never suppressed. Verdicts are appended to the committed decision log.",
            json!({
                "fingerprint": { "type": "string", "description": "A fingerprint or an unambiguous prefix." },
                "verdict": { "type": "string", "enum": ["confirmed", "rejected", "deferred"] },
                "reason": { "type": "string", "enum": ["fixed", "accepted-debt", "false-positive", "intentional-exception", "scope-too-broad", "project-allowed", "not-worth-fixing"] },
                "note": { "type": "string", "description": "Why, written for the next reader." },
                "seen": { "type": "string", "description": "The finding's `last_seen` as you read it; the verdict is refused if it was seen again since." }
            }),
            &["fingerprint", "verdict"],
        ),
        tool(
            "review_history",
            "Every verdict recorded on a finding, oldest first.",
            json!({ "fingerprint": { "type": "string" } }),
            &["fingerprint"],
        ),
        tool(
            "decision_create",
            "Add a project-local decision under .lighthouse/decisions. `id` is `local/<name>`; `spec` is the spec of a Decision (title, intent, scope {subject}, requirement with MUST/SHOULD, enforcement, evidence, and a `check` of `type: cel` with select, where as a CEL expression that is true for a violation, message and evidence); it may be an object or a YAML/JSON string. Nothing is written unless the candidate validates, compiles and every example passes the whole engine; a rejection leaves the project untouched. The decision also needs the `local` plugin in lighthouse.toml to run in `check`.",
            json!({
                "id": { "type": "string", "description": "`local/<name>`." },
                "spec": { "type": ["object", "string"], "description": "The Decision spec (YAML or JSON text, or an object)." },
                "examples": { "type": ["array", "string"], "description": "Examples appended to the spec's own: at least one valid and one invalid per language the project runs; each {name, language, kind: valid|invalid, files: [{path, body}], expect: [{line, message?}]}." }
            }),
            &["id", "spec", "examples"],
        ),
        tool(
            "decision_update",
            "Change a project-local decision, or adjust a bundled one through the project's override file. `patch` is a JSON merge patch over the spec (null removes a key). For a bundled decision only severity, exceptions, options (new defaults), languages and examples can change. The same gate as decision_create applies, and nothing is written when it fails.",
            json!({
                "id": { "type": "string", "description": "A local decision or any catalog decision." },
                "patch": { "type": ["object", "string"], "description": "JSON merge patch, as an object or JSON/YAML text." }
            }),
            &["id", "patch"],
        ),
        tool(
            "decision_test",
            "Run the examples of checked decisions through the whole engine, in each language the project's plugins provide. `ok` is false when any example fails.",
            json!({ "ids": { "type": "array", "items": { "type": "string" }, "description": "Decision ids; default every checked decision." } }),
            &[],
        ),
    ]
    .into()
}

fn tool(
    name: &'static str,
    description: &'static str,
    properties: Value,
    required: &[&str],
) -> Tool {
    let schema = json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    });
    let Value::Object(schema): Value = schema else {
        unreachable!("a schema literal is an object")
    };
    Tool::new(name, description, JsonObject::from(schema))
}
