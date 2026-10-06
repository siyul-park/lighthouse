use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::{Diagnostic, Incomplete, Severity};
use serde::Serialize;

const SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const FINGERPRINT_KEY: &str = "lighthouse/v1";
const SRCROOT: &str = "%SRCROOT%";

#[derive(Serialize)]
struct Log<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: [Run<'a>; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Run<'a> {
    tool: Tool<'a>,
    original_uri_base_ids: BTreeMap<&'static str, BaseId>,
    invocations: [Invocation<'a>; 1],
    results: Vec<SarifResult<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Invocation<'a> {
    execution_successful: bool,
    tool_execution_notifications: Vec<Notification<'a>>,
}

#[derive(Serialize)]
struct Notification<'a> {
    level: &'static str,
    message: Message<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    locations: Vec<NotificationLocation>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationLocation {
    physical_location: ArtifactOnly,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactOnly {
    artifact_location: Artifact,
}

#[derive(Serialize)]
struct Tool<'a> {
    driver: Driver<'a>,
}

#[derive(Serialize)]
struct Driver<'a> {
    name: &'static str,
    version: &'static str,
    rules: Vec<RuleRef<'a>>,
}

#[derive(Serialize)]
struct RuleRef<'a> {
    id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult<'a> {
    rule_id: &'a str,
    level: &'static str,
    message: Message<'a>,
    locations: [Location; 1],
    partial_fingerprints: BTreeMap<&'static str, &'a str>,
}

#[derive(Serialize)]
struct Message<'a> {
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    physical_location: Physical,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Physical {
    artifact_location: Artifact,
    region: Region,
}

#[derive(Serialize)]
struct BaseId {
    description: Message<'static>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Artifact {
    uri: String,
    uri_base_id: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

fn notification(item: &Incomplete) -> Notification<'_> {
    Notification {
        level: "error",
        message: Message { text: &item.reason },
        locations: item
            .path
            .iter()
            .map(|path| NotificationLocation {
                physical_location: ArtifactOnly {
                    artifact_location: Artifact {
                        uri: encode(&path.to_string_lossy().replace('\\', "/")),
                        uri_base_id: SRCROOT,
                    },
                },
            })
            .collect(),
    }
}

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warn => "warning",
        Severity::Review | Severity::Info => "note",
    }
}

/// Percent-encodes everything but unreserved characters and `/`.
fn encode(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(char::from(b));
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn render(diagnostics: &[Diagnostic], incomplete: &[Incomplete]) -> String {
    let rules: BTreeSet<&str> = diagnostics.iter().map(|d| d.rule_id.as_str()).collect();
    let log = Log {
        schema: SCHEMA,
        version: "2.1.0",
        runs: [Run {
            original_uri_base_ids: BTreeMap::from([(
                SRCROOT,
                BaseId {
                    description: Message {
                        text: "directory containing lighthouse.toml",
                    },
                },
            )]),
            invocations: [Invocation {
                execution_successful: incomplete.is_empty(),
                tool_execution_notifications: incomplete.iter().map(notification).collect(),
            }],
            tool: Tool {
                driver: Driver {
                    name: "lighthouse",
                    version: env!("CARGO_PKG_VERSION"),
                    rules: rules.into_iter().map(|id| RuleRef { id }).collect(),
                },
            },
            results: diagnostics
                .iter()
                .map(|d| SarifResult {
                    rule_id: &d.rule_id,
                    level: level(d.severity),
                    message: Message { text: &d.message },
                    locations: [Location {
                        physical_location: Physical {
                            artifact_location: Artifact {
                                uri: encode(&d.file.to_string_lossy().replace('\\', "/")),
                                uri_base_id: SRCROOT,
                            },
                            region: Region {
                                start_line: d.span.start.line,
                                start_column: d.span.start.col,
                                end_line: d.span.end.line,
                                end_column: d.span.end.col,
                            },
                        },
                    }],
                    partial_fingerprints: BTreeMap::from([(
                        FINGERPRINT_KEY,
                        d.fingerprint.as_str(),
                    )]),
                })
                .collect(),
        }],
    };
    let mut out = serde_json::to_string_pretty(&log).expect("sarif serializes");
    out.push('\n');
    out
}
