//! The JSON Schema of what each command prints with `--format json`, and what
//! the MCP tool of that command returns as `structuredContent`. Tests validate
//! real outputs against them, so a schema cannot drift from its command.

use std::collections::BTreeMap;

use lighthouse_resource::{SCHEMA_URL_BASE, schema_file};
use serde_json::{Value, json};

/// Every output kind with its schema, by kind name.
pub fn kinds() -> BTreeMap<&'static str, Value> {
    [
        ("CheckReport", check_report()),
        ("FixReport", fix_report()),
        ("DecisionList", decision_list()),
        ("DecisionShow", decision_show()),
        ("TestReport", test_report()),
        ("AuthorReport", author_report()),
        ("EvalReport", eval_report()),
        ("ReviewList", review_list()),
        ("Finding", finding()),
        ("ResolveReport", resolve_report()),
        ("JudgmentHistory", judgment_history()),
        ("ModelList", model_list()),
        ("TrainReport", train_report()),
    ]
    .into_iter()
    .map(|(kind, schema)| (kind, document(kind, schema)))
    .collect()
}

/// The `$id` and the title of a kind's schema.
fn document(kind: &str, mut schema: Value) -> Value {
    schema["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
    schema["$id"] = json!(format!("{SCHEMA_URL_BASE}/{}", schema_file(kind)));
    schema["title"] = json!(kind);
    schema
}

fn string() -> Value {
    json!({ "type": "string" })
}

fn integer() -> Value {
    json!({ "type": "integer", "minimum": 0 })
}

fn strings() -> Value {
    json!({ "type": "array", "items": string() })
}

fn nullable(inner: Value) -> Value {
    json!({ "anyOf": [inner, { "type": "null" }] })
}

fn list_of(item: Value) -> Value {
    json!({ "type": "array", "items": item })
}

/// An object with these properties, all of `required` present, and room for
/// more: the schema describes what a program may rely on.
fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn status() -> Value {
    json!({ "enum": ["clean", "findings", "incomplete", "review"] })
}

fn check_report() -> Value {
    object(
        json!({
            "status": status(),
            "counts": object(json!({
                "error": integer(), "warn": integer(), "review": integer(),
                "info": integer(), "suppressed": integer(), "allowed": integer()
            }), &["error", "warn", "review"]),
            "groups": list_of(json!({ "type": "object" })),
            "findings": list_of(json!({ "type": "object" })),
            "incomplete": list_of(json!({})),
            "omitted": {
                "oneOf": [
                    integer(),
                    object(json!({ "groups": integer(), "findings": integer() }), &["groups", "findings"])
                ]
            },
            "resolve": string(),
            "judgments": json!({}),
            "summary": object(json!({
                "status": json!({ "enum": ["clean", "findings", "incomplete"] }),
                "errors": integer(), "warnings": integer(), "infos": integer(),
                "reviews": integer(), "incomplete": integer(),
                "suppressed": integer(), "allowed": integer()
            }), &["status"]),
            "reasons": json!({}),
            "messages": strings()
        }),
        &["status"],
    )
}

fn fix_report() -> Value {
    object(
        json!({
            "dryRun": { "type": "boolean" },
            "diff": string(),
            "applied": list_of(object(json!({
                "fingerprint": string(), "rule": string(), "fixer": string(),
                "safety": string(), "description": string(), "files": strings()
            }), &["fingerprint", "rule", "fixer", "safety"])),
            "declined": list_of(object(json!({
                "fingerprint": string(), "rule": string(), "path": string(),
                "line": integer(), "reason": string()
            }), &["fingerprint", "rule", "reason"])),
            "rounds": integer(),
            "messages": strings()
        }),
        &[
            "dryRun", "diff", "applied", "declined", "rounds", "messages",
        ],
    )
}

fn decision_list() -> Value {
    object(
        json!({
            "decisions": list_of(object(json!({
                "id": string(), "status": string(), "severity": string(),
                "title": string(), "enabled": { "type": "boolean" }
            }), &["id", "status", "severity", "title", "enabled"])),
            "omitted": integer()
        }),
        &["decisions", "omitted"],
    )
}

fn decision_show() -> Value {
    object(
        json!({ "id": string(), "markdown": string() }),
        &["id", "markdown"],
    )
}

fn test_report() -> Value {
    object(
        json!({
            "ok": { "type": "boolean" }, "decisions": integer(), "languages": integer(),
            "runs": integer(), "failures": strings()
        }),
        &["ok", "decisions", "languages", "runs", "failures"],
    )
}

fn author_report() -> Value {
    object(
        json!({
            "id": string(), "path": string(), "created": { "type": "boolean" },
            "tested": object(json!({ "runs": integer(), "languages": integer() }), &["runs", "languages"]),
            "localPluginListed": { "type": "boolean" }, "note": string()
        }),
        &[
            "id",
            "path",
            "created",
            "tested",
            "localPluginListed",
            "note",
        ],
    )
}

fn rate() -> Value {
    nullable(object(
        json!({ "hits": integer(), "of": integer(), "value": { "type": "number" },
                "low": { "type": "number" }, "high": { "type": "number" } }),
        &["hits", "of", "value", "low", "high"],
    ))
}

fn side() -> Value {
    object(
        json!({
            "check": string(),
            "metrics": object(json!({
                "precision": rate(), "fpr": rate(), "recall": rate(), "fnr": rate(),
                "selector_precision": rate(), "coverage": rate(), "abstention": rate()
            }), &[]),
            "model_decided": integer(), "reviews": integer(), "calls": integer(),
            "cost": { "type": "number" }
        }),
        &[
            "check",
            "metrics",
            "model_decided",
            "reviews",
            "calls",
            "cost",
        ],
    )
}

fn eval_report() -> Value {
    object(
        json!({
            "decision": string(), "labelled": integer(),
            "policy": object(json!({ "precision": { "type": "number" } }), &["precision"]),
            "current": side(), "candidate": nullable(side()), "agreement": rate(),
            "meets_policy": nullable(json!({ "type": "boolean" })),
            "caveat": string(), "audit": nullable(string())
        }),
        &["decision", "labelled", "policy", "current", "caveat"],
    )
}

fn review_list() -> Value {
    object(
        json!({
            "groups": list_of(json!({ "type": "object" })),
            "tasks": list_of(json!({ "type": "object" })),
            "omitted": {
                "oneOf": [
                    integer(),
                    object(json!({ "groups": integer(), "findings": integer() }), &["groups", "findings"])
                ]
            },
            "resolve": string(), "judgments": json!({}),
            "notices": strings(), "note": string()
        }),
        &[],
    )
}

fn finding() -> Value {
    object(
        json!({
            "fingerprint": string(), "ruleId": string(),
            "severity": string(), "authoredSeverity": string(),
            "path": string(), "locator": { "type": "object" },
            "symbol": nullable(string()),
            "firstSeen": string(), "lastSeen": string(),
            "resolvedAt": nullable(string()), "inactiveAt": nullable(string()),
            "reopened": integer(), "message": string(),
            "evidence": json!({}), "facts": json!({}), "options": json!({}),
            "judgment": nullable(string()), "standing": nullable(string()),
            "narrowing": { "type": "boolean" }
        }),
        &[
            "fingerprint",
            "ruleId",
            "severity",
            "path",
            "message",
            "lastSeen",
        ],
    )
}

fn resolve_report() -> Value {
    object(
        json!({
            "recorded": object(json!({
                "fingerprint": string(), "decision": string(), "judgment": string(),
                "suppression": nullable(json!({ "type": "object" })),
                "wasAttributedTo": json!({})
            }), &["fingerprint", "decision", "judgment"]),
            "standing": nullable(string()),
            "warnings": strings(),
            "catalogError": nullable(string())
        }),
        &["recorded", "warnings"],
    )
}

fn judgment_history() -> Value {
    object(
        json!({ "fingerprint": string(), "events": list_of(json!({ "type": "object" })) }),
        &["fingerprint", "events"],
    )
}

fn model_list() -> Value {
    object(
        json!({
            "models": list_of(object(json!({
                "model": string(), "decision": nullable(string()),
                "binding": { "enum": ["classification", "rule", "attached"] },
                "remote": { "type": "boolean" }
            }), &["model", "decision", "binding", "remote"])),
            "omitted": integer()
        }),
        &["models", "omitted"],
    )
}

fn train_report() -> Value {
    object(
        json!({
            "decision": string(), "model": string(), "subjects": integer(),
            "by_source": { "type": "object", "additionalProperties": integer() },
            "unresolved": integer(), "dataset": string(), "training_file": string(),
            "result": nullable(json!({ "type": "object" }))
        }),
        &[
            "decision",
            "model",
            "subjects",
            "unresolved",
            "dataset",
            "training_file",
        ],
    )
}
