//! The tools the server offers: name, description and the JSON Schema of the
//! arguments. `pattern_similar`, `rule_proposals` and `fix` are reserved for
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
            "explain",
            "Explain a pattern or rule: intent, requirement, examples, exceptions and status, as Markdown.",
            json!({ "id": { "type": "string", "description": "A pattern or rule id such as `design/exported-doc`." } }),
            &["id"],
        ),
        tool(
            "rule_list",
            "List the rules with severity, title and whether this project's configuration enables them.",
            json!({ "all": { "type": "boolean", "description": "Include every catalog pattern, implemented or not." } }),
            &[],
        ),
        tool(
            "review_tasks",
            "List remembered findings that wait for a judgment (by default open findings of severity `review`), each with the fingerprint and `last_seen` that `review_resolve` needs.",
            json!({
                "status": { "type": "string", "enum": ["open", "suppressed", "narrowing", "inactive", "resolved", "all"], "description": "Default `open`." },
                "rule": { "type": "string", "description": "Only this rule." },
                "tier": { "type": "string", "enum": ["review", "all"], "description": "Default `review`: findings of severity review, which ask for a judgment. `all` lists every severity." },
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
            "rule_create",
            "Add a project-local declarative rule under .lighthouse/rules. The pattern (id `local/<name>`, title, intent, scope, requirement with MUST/SHOULD, enforcement, evidence) and the rule (select, where as a CEL expression that is true for a violation, message, evidence) may each be an object or a YAML/JSON string. Nothing is written unless the candidate validates, compiles and every example passes the whole engine; a rejection leaves the project untouched. The rule also needs the `local` plugin in lighthouse.toml to run in `check`.",
            json!({
                "pattern": { "type": ["object", "string"], "description": "The pattern definition (YAML or JSON text, or an object)." },
                "rule": { "type": ["object", "string"], "description": "The declarative rule; omit when `pattern` has a `rule` key." },
                "examples": { "type": ["array", "string"], "description": "Examples appended to the pattern's own: at least one valid and one invalid per language the project runs; each {name, language, kind: valid|invalid, files: [{path, body}], expect: [{line, message?}]}." }
            }),
            &["pattern", "examples"],
        ),
        tool(
            "rule_update",
            "Change a project-local rule, or adjust a bundled one through the project's overlay file. `patch` is a JSON merge patch (null removes a key). For a bundled pattern only severity, exceptions, options, tuning and examples can change. The same gate as rule_create applies, and nothing is written when it fails.",
            json!({
                "id": { "type": "string", "description": "A local rule or any catalog pattern." },
                "patch": { "type": ["object", "string"], "description": "JSON merge patch, as an object or JSON/YAML text." }
            }),
            &["id", "patch"],
        ),
        tool(
            "rule_test",
            "Run the examples of implemented patterns through the whole engine, in each language the project's plugins provide. `ok` is false when any example fails.",
            json!({ "ids": { "type": "array", "items": { "type": "string" }, "description": "Pattern ids; default every implemented pattern." } }),
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
