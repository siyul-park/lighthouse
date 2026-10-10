use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::{Diagnostic, Incomplete, Position, Severity, Suppressed, Suppression};
use lighthouse_spec::{Catalog, Decision, help_path};

use crate::fix::{FixEdit, ProposedFix, utf16_position};
use serde::Serialize;

const SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const FINGERPRINT_KEY: &str = "lighthouse/v1";
const SRCROOT: &str = "%SRCROOT%";
/// Where the generated decision pages are served; a rule's `helpUri` is a
/// page below it.
const DOCS_URL_BASE: &str = "https://github.com/siyul-park/lighthouse/blob/main";

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

/// A rule of the driver: the decision a finding cites, when the catalog has it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleRef<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_description: Option<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_description: Option<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    help_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_configuration: Option<Configuration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<RuleProperties<'a>>,
}

#[derive(Serialize)]
struct Configuration {
    level: &'static str,
}

#[derive(Serialize)]
struct RuleProperties<'a> {
    status: String,
    tags: Vec<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult<'a> {
    rule_id: &'a str,
    rule_index: usize,
    level: &'static str,
    message: Message<'a>,
    locations: [Location; 1],
    partial_fingerprints: BTreeMap<&'static str, &'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fixes: Vec<SarifFix<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    suppressions: Vec<SarifSuppression<'a>>,
}

/// A SARIF `suppression`: `inSource` for a directive in the code, `external`
/// for a recorded decision to leave the finding in place.
#[derive(Serialize)]
struct SarifSuppression<'a> {
    kind: &'static str,
    /// Left out when `accepted`, which is what SARIF takes it to be.
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
    /// Left out when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    justification: Option<&'a str>,
}

/// A SARIF `fix`: what to change, as replacements of regions per artifact.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifFix<'a> {
    description: Message<'a>,
    artifact_changes: Vec<ArtifactChange>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactChange {
    artifact_location: Artifact,
    replacements: Vec<Replacement>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Replacement {
    deleted_region: Region,
    inserted_content: Content,
}

#[derive(Serialize)]
struct Content {
    text: String,
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

/// Renders a SARIF 2.1.0 log with one result per diagnostic, one per
/// suppressed finding (carrying its `suppressions`), and a tool
/// notification per incomplete entry; an incomplete analysis marks the
/// invocation unsuccessful. A finding's rule is the decision it cites: with
/// the `catalog` the rule carries the decision's title, requirement, default
/// level and a `helpUri` to its entry in the generated docs; a result carries
/// the fingerprint of the finding as a partial fingerprint, and the fix
/// proposed for it (by fingerprint in `fixes`) as a SARIF `fix`.
pub fn render(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    catalog: Option<&Catalog>,
    fixes: Option<&BTreeMap<String, ProposedFix>>,
    suppressed: &[Suppressed],
    sources: Option<&BTreeMap<String, String>>,
) -> String {
    let ids: BTreeSet<&str> = diagnostics
        .iter()
        .chain(suppressed.iter().map(|s| &s.diagnostic))
        .map(|d| d.rule_id.as_str())
        .collect();
    let rules: Vec<&str> = ids.into_iter().collect();
    let log =
        Log {
            schema: SCHEMA,
            version: "2.1.0",
            runs: [Run {
                original_uri_base_ids: BTreeMap::from([(
                    SRCROOT,
                    BaseId {
                        description: Message {
                            text: "the project root, where Lighthouse was run",
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
                        rules: rules
                            .iter()
                            .map(|id| rule_ref(id, catalog.and_then(|c| c.decision(id))))
                            .collect(),
                    },
                },
                results: diagnostics
                    .iter()
                    .map(|d| {
                        let fix = fixes.and_then(|f| f.get(d.fingerprint.as_str()));
                        result(d, &rules, fix, None, sources)
                    })
                    .chain(suppressed.iter().map(|s| {
                        result(&s.diagnostic, &rules, None, Some(&s.suppression), sources)
                    }))
                    .collect(),
            }],
        };
    let mut out = serde_json::to_string_pretty(&log).expect("sarif serializes");
    out.push('\n');
    out
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
                    artifact_location: artifact(path),
                },
            })
            .collect(),
    }
}

fn rule_ref<'a>(id: &'a str, decision: Option<&'a Decision>) -> RuleRef<'a> {
    let Some(decision) = decision else {
        return RuleRef {
            id,
            name: None,
            short_description: None,
            full_description: None,
            help_uri: None,
            default_configuration: None,
            properties: None,
        };
    };
    RuleRef {
        id,
        name: Some(decision.short_name()),
        short_description: Some(Message {
            text: &decision.title,
        }),
        full_description: Some(Message {
            text: &decision.requirement,
        }),
        help_uri: Some(format!("{DOCS_URL_BASE}/{}", help_path(decision))),
        default_configuration: decision
            .severity()
            .map(|s| Configuration { level: level(s) }),
        properties: Some(RuleProperties {
            status: decision.status.to_string(),
            tags: vec![decision.pack()],
        }),
    }
}

fn result<'a>(
    d: &'a Diagnostic,
    rules: &[&str],
    fix: Option<&'a ProposedFix>,
    suppression: Option<&'a Suppression>,
    sources: Option<&BTreeMap<String, String>>,
) -> SarifResult<'a> {
    SarifResult {
        rule_id: &d.rule_id,
        rule_index: rules
            .iter()
            .position(|r| *r == d.rule_id)
            .expect("every result's rule is in the driver's rules"),
        level: level(d.severity),
        message: Message { text: &d.message },
        locations: [Location {
            physical_location: Physical {
                artifact_location: artifact(&d.file),
                region: region(d, sources),
            },
        }],
        partial_fingerprints: BTreeMap::from([(FINGERPRINT_KEY, d.fingerprint.as_str())]),
        fixes: fix.map(sarif_fix).into_iter().collect(),
        suppressions: suppression
            .map(|s| SarifSuppression {
                kind: s.kind.as_str(),
                status: (!s.in_force()).then(|| s.status.as_str()),
                justification: Some(s.justification.as_str()).filter(|j| !j.trim().is_empty()),
            })
            .into_iter()
            .collect(),
    }
}

/// The region of a finding. SARIF columns count UTF-16 code units, so with
/// the text of the file the byte columns of the span are converted.
fn region(d: &Diagnostic, sources: Option<&BTreeMap<String, String>>) -> Region {
    let key = d.file.to_string_lossy().replace('\\', "/");
    let text = sources.and_then(|s| s.get(&key));
    let at = |p: Position| text.map_or(p, |t| utf16_position(t, p));
    let (start, end) = (at(d.span.start), at(d.span.end));
    Region {
        start_line: start.line,
        start_column: start.col,
        end_line: end.line,
        end_column: end.col,
    }
}

fn sarif_fix(fix: &ProposedFix) -> SarifFix<'_> {
    SarifFix {
        description: Message {
            text: &fix.description,
        },
        artifact_changes: fix
            .files
            .iter()
            .map(|file| ArtifactChange {
                artifact_location: artifact(Path::new(&file.path)),
                replacements: file.edits.iter().map(replacement).collect(),
            })
            .collect(),
    }
}

fn replacement(edit: &FixEdit) -> Replacement {
    Replacement {
        deleted_region: Region {
            start_line: edit.start.line,
            start_column: edit.start.col,
            end_line: edit.end.line,
            end_column: edit.end.col,
        },
        inserted_content: Content {
            text: edit.text.clone(),
        },
    }
}

fn artifact(path: &Path) -> Artifact {
    Artifact {
        uri: encode(&path.to_string_lossy().replace('\\', "/")),
        uri_base_id: SRCROOT,
    }
}

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warn => "warning",
        Severity::Info => "note",
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
