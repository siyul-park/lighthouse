use std::{cmp::Reverse, collections::BTreeMap, fmt::Write, path::Path};

use lighthouse_model::{Fingerprint, Judgment, Severity, squash};
use lighthouse_spec::Catalog;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{
    descriptor::Descriptor,
    expected::{ExampleKey, expected},
    text::{bounded, evidence, pairs},
};

/// Longest a symbol's own name may be to count as mentioned by a message.
const NAME_MIN: usize = 3;
/// How a judgment is recorded, said once per report.
const RESOLVE: &str = "review_resolve {fingerprint, judgment, suppress?, reason?}";

/// What each judgment says, and how to leave a finding that is right in place.
const MEANINGS: [(Judgment, &str); 3] = [
    (
        Judgment::Fail,
        "the finding is right; add `suppress` with a justification to leave it in place",
    ),
    (Judgment::Pass, "false positive: the code conforms"),
    (
        Judgment::NotApplicable,
        "the decision does not apply here; a hint to narrow it",
    ),
];

/// Findings grouped by decision, as a program reads them: a group says what
/// its decision says once, and a finding is one row of `[line:col, message,
/// fingerprint]` with a last object for what sets it apart.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Groups {
    pub groups: Vec<Value>,
    /// What the limit left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<Omitted>,
    /// How a judgment is recorded, said once for all groups; present when a
    /// shown finding asks for review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolve: Option<&'static str>,
    /// What each judgment says; present with `resolve`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judgments: Option<Judgments>,
}

/// How many groups and findings a report left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Omitted {
    pub groups: usize,
    pub findings: usize,
}

/// What each judgment says, by judgment.
pub type Judgments = BTreeMap<&'static str, &'static str>;

/// One finding as the compact shape sees it, wherever it comes from: a fresh
/// diagnostic or a remembered finding.
#[derive(Debug, Clone)]
pub struct Entry {
    pub decision: String,
    pub severity: Severity,
    /// The severity the decision authored; shown only when it differs.
    pub authored: Severity,
    /// Whether the finding asks for review.
    pub review: bool,
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub symbol: Option<String>,
    pub evidence: Value,
    /// More evidence-like facts, shared by a group when they agree.
    pub attributes: Map<String, Value>,
    /// Why the finding is reported although a judgment was recorded on it.
    pub note: Option<String>,
    pub fingerprint: String,
    /// Subject facts: `language`, `kind` and `visibility` pick the expected
    /// structure.
    pub facts: Option<Value>,
}

/// What shapes the groups.
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupOptions<'a> {
    /// The decisions findings cite.
    pub catalog: Option<&'a Catalog>,
    /// Keep at most this many findings.
    pub limit: Option<usize>,
    /// The groups are data for a program: hints name tool calls, not
    /// command lines.
    pub tool_form: bool,
    /// How the check and the fix of each decision run, by decision id.
    pub descriptors: Option<&'a BTreeMap<String, Descriptor>>,
}

/// Findings grouped by decision, then by file, as many as fit a limit.
#[derive(Debug, Clone)]
pub struct Grouped {
    groups: Vec<Group>,
    omitted_groups: usize,
    omitted_findings: usize,
}

/// The findings of one decision at one severity.
#[derive(Debug, Clone)]
struct Group {
    decision: String,
    severity: Severity,
    authored: Option<Severity>,
    /// Some finding of the group asks for review; when only some do,
    /// their instances say which.
    review: bool,
    requirement: Option<String>,
    /// The expected structure by language, once for all findings of it.
    expected: Vec<(Option<String>, String)>,
    /// How the check and the fix of the decision run, and the fingerprint of
    /// its first finding, which the text offers to brief.
    descriptor: Option<(Descriptor, String)>,
    tool_form: bool,
    /// The evidence every finding of the group agrees on.
    evidence: Map<String, Value>,
    files: BTreeMap<String, Vec<Instance>>,
}

/// One finding: where, what, which, and what sets it apart from the group.
#[derive(Debug, Clone)]
struct Instance {
    line: u32,
    col: u32,
    message: String,
    fingerprint: String,
    evidence: Map<String, Value>,
    note: Option<String>,
    /// Asks for review; set only in a mixed group.
    review: bool,
}

impl Grouped {
    /// The groups of `entries`: errors first, then the larger ones. At most
    /// `limit` findings are kept, in that order; what does not fit is counted.
    /// Fingerprint prefixes are unique among all `entries`, shown or not.
    pub fn of(entries: Vec<Entry>, options: &GroupOptions) -> Self {
        let limit = options.limit;
        let fingerprints: Vec<&str> = entries.iter().map(|e| e.fingerprint.as_str()).collect();
        let widths = Fingerprint::prefix_widths(&fingerprints);
        let mut buckets: BTreeMap<(Severity, &str), Vec<(&Entry, usize)>> = BTreeMap::new();
        for (entry, width) in entries.iter().zip(widths) {
            buckets
                .entry((entry.severity, entry.decision.as_str()))
                .or_default()
                .push((entry, width));
        }
        let mut ordered: Vec<_> = buckets.into_iter().collect();
        ordered.sort_by_key(|((severity, rule), rows)| (*severity, Reverse(rows.len()), *rule));
        let mut room = limit.unwrap_or(usize::MAX);
        let mut grouped = Self {
            groups: Vec::new(),
            omitted_groups: 0,
            omitted_findings: 0,
        };
        for (_, rows) in ordered {
            let take = rows.len().min(room);
            room -= take;
            grouped.omitted_findings += rows.len() - take;
            if take == 0 {
                grouped.omitted_groups += 1;
            } else {
                grouped.groups.push(Group::build(rows, take, options));
            }
        }
        grouped
    }

    /// Whether any shown finding asks for review.
    pub fn asks_review(&self) -> bool {
        self.groups.iter().any(|g| g.review)
    }

    /// The groups as data for a program.
    pub fn data(&self) -> Groups {
        let asks_review = self.asks_review();
        Groups {
            groups: self.groups.iter().map(Group::to_json).collect(),
            omitted: (self.omitted_findings > 0).then_some(Omitted {
                groups: self.omitted_groups,
                findings: self.omitted_findings,
            }),
            resolve: asks_review.then_some(RESOLVE),
            judgments: asks_review.then(judgments),
        }
    }

    /// One header per group, its expected structure and shared evidence, then
    /// a line per finding; a last line says how many were left out.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for group in &self.groups {
            group.write_text(&mut out);
        }
        if self.omitted_findings > 0 {
            let _ = writeln!(
                out,
                "... {} more finding(s) not shown (raise --limit)",
                self.omitted_findings
            );
        }
        out
    }
}

impl Group {
    /// The first `take` of `rows` by file and position.
    fn build(mut rows: Vec<(&Entry, usize)>, take: usize, options: &GroupOptions) -> Self {
        rows.sort_by(|(a, _), (b, _)| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        rows.truncate(take);
        let first = rows[0].0;
        let review = rows.iter().any(|(entry, _)| entry.review);
        let mixed = review && rows.iter().any(|(entry, _)| !entry.review);
        let decision = options.catalog.and_then(|c| c.decision(&first.decision));
        let attributes: Vec<Map<String, Value>> =
            rows.iter().map(|(entry, _)| attributes(entry)).collect();
        let shared: Map<String, Value> = attributes[0]
            .iter()
            .filter(|(key, value)| attributes.iter().all(|a| a.get(*key) == Some(*value)))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut expecteds: Vec<(Option<String>, String)> = Vec::new();
        if let Some(decision) = decision {
            for (entry, _) in &rows {
                let subject = ExampleKey::new(entry.facts.as_ref());
                let language = subject.language.map(str::to_owned);
                if expecteds.iter().any(|(l, _)| *l == language) {
                    continue;
                }
                if let Some(found) = expected(decision, &subject, Path::new(&entry.path)) {
                    expecteds.push((language, found.excerpt));
                }
            }
        }
        let mut files: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
        for ((entry, width), mut attributes) in rows.into_iter().zip(attributes) {
            attributes.retain(|key, _| !shared.contains_key(key));
            files.entry(entry.path.clone()).or_default().push(Instance {
                line: entry.line,
                col: entry.col,
                message: entry.message.clone(),
                fingerprint: entry
                    .fingerprint
                    .get(..width)
                    .unwrap_or(&entry.fingerprint)
                    .to_owned(),
                evidence: attributes,
                note: entry.note.clone(),
                review: mixed && entry.review,
            });
        }
        Self {
            decision: first.decision.clone(),
            severity: first.severity,
            authored: (first.authored != first.severity).then_some(first.authored),
            review,
            requirement: decision.map(|d| squash(&d.requirement)),
            expected: expecteds,
            descriptor: options
                .descriptors
                .and_then(|all| all.get(&first.decision))
                .map(|descriptor| (descriptor.clone(), first.fingerprint.clone())),
            tool_form: options.tool_form,
            evidence: shared,
            files,
        }
    }

    fn to_json(&self) -> Value {
        let mut group = Map::new();
        group.insert("decision".to_owned(), json!(self.decision));
        group.insert("severity".to_owned(), json!(self.severity));
        if let Some(authored) = self.authored {
            group.insert("authored".to_owned(), json!(authored));
        }
        if self.review {
            group.insert("review".to_owned(), json!(true));
        }
        if let Some(requirement) = &self.requirement {
            group.insert("requirement".to_owned(), json!(requirement));
        }
        match self.expected.as_slice() {
            [] => {}
            [(_, excerpt)] => {
                group.insert("expected".to_owned(), json!(excerpt));
            }
            many => {
                let by_language: Map<String, Value> = many
                    .iter()
                    .map(|(language, excerpt)| {
                        (language.clone().unwrap_or_default(), json!(excerpt))
                    })
                    .collect();
                group.insert("expected".to_owned(), Value::Object(by_language));
            }
        }
        if let Some((descriptor, _)) = &self.descriptor {
            group.insert("check".to_owned(), json!(descriptor.check));
            if let Some(fix) = &descriptor.fix {
                group.insert("fix".to_owned(), json!(fix));
            }
            if descriptor.brief {
                group.insert("brief".to_owned(), json!(true));
            }
        }
        if !self.evidence.is_empty() {
            group.insert("evidence".to_owned(), Value::Object(self.evidence.clone()));
        }
        let files: Map<String, Value> = self
            .files
            .iter()
            .map(|(path, instances)| {
                let rows = instances.iter().map(Instance::to_json).collect();
                (path.clone(), Value::Array(rows))
            })
            .collect();
        group.insert("files".to_owned(), Value::Object(files));
        Value::Object(group)
    }

    fn write_text(&self, out: &mut String) {
        let _ = write!(out, "{} {}", self.decision, self.severity);
        if let Some(authored) = self.authored {
            let _ = write!(out, " (authored {authored})");
        }
        if self.review {
            out.push_str(" [review]");
        }
        if let Some(requirement) = &self.requirement {
            let _ = write!(out, " \u{2014} {requirement}");
        }
        out.push('\n');
        for (language, excerpt) in &self.expected {
            let label = match (language, self.expected.len()) {
                (Some(language), 2..) => format!("expected ({language}):"),
                _ => "expected:".to_owned(),
            };
            if excerpt.contains('\n') {
                let _ = writeln!(out, "  {label}");
                for line in excerpt.lines() {
                    let _ = writeln!(out, "{}", format!("    {line}").trim_end());
                }
            } else {
                let _ = writeln!(out, "  {label} {excerpt}");
            }
        }
        if let Some((descriptor, fingerprint)) = &self.descriptor {
            let _ = writeln!(out, "  {}", descriptor.line(fingerprint, self.tool_form));
        }
        if let Some(shared) = pairs(&self.evidence) {
            let _ = writeln!(out, "  evidence: {shared}");
        }
        for (path, instances) in &self.files {
            for instance in instances {
                instance.write_text(path, out);
            }
        }
    }
}

impl Instance {
    fn to_json(&self) -> Value {
        let mut row = vec![
            json!(format!("{}:{}", self.line, self.col)),
            json!(self.message),
            json!(self.fingerprint),
        ];
        let mut varying = Map::new();
        if !self.evidence.is_empty() {
            varying.insert("evidence".to_owned(), Value::Object(self.evidence.clone()));
        }
        if let Some(note) = &self.note {
            varying.insert("note".to_owned(), json!(note));
        }
        if self.review {
            varying.insert("review".to_owned(), json!(true));
        }
        if !varying.is_empty() {
            row.push(Value::Object(varying));
        }
        Value::Array(row)
    }

    fn write_text(&self, path: &str, out: &mut String) {
        let _ = write!(
            out,
            "  {path}:{}:{} {} {}",
            self.line, self.col, self.message, self.fingerprint
        );
        if self.review {
            out.push_str(" [review]");
        }
        if let Some(varying) = pairs(&self.evidence) {
            let _ = write!(out, " {{{varying}}}");
        }
        if let Some(note) = &self.note {
            let _ = write!(out, " (note: {note})");
        }
        out.push('\n');
    }
}

pub(crate) fn judgments() -> Judgments {
    MEANINGS.iter().map(|(j, m)| (j.as_str(), *m)).collect()
}

/// The command line that records a judgment on the finding `fingerprint`
/// names.
pub(crate) fn resolve_command(fingerprint: &str) -> String {
    format!(
        "lighthouse review resolve {fingerprint} --judgment <pass|fail|notApplicable> [--suppress <justification>] [--reason <text>] --attributed-to agent:<id>"
    )
}

pub(crate) fn judgments_line() -> String {
    MEANINGS
        .iter()
        .map(|(j, m)| format!("{j}={m}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The evidence of a finding that may be shared or may set it apart: its
/// bounded evidence, the symbol unless the message already names it, and the
/// entry's own attributes.
fn attributes(entry: &Entry) -> Map<String, Value> {
    let mut map = match evidence(&entry.evidence, entry.symbol.as_deref()) {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => Map::from_iter([("evidence".to_owned(), bounded(&other))]),
    };
    if let Some(symbol) = entry
        .symbol
        .as_deref()
        .filter(|s| !mentions(&entry.message, s))
    {
        map.insert("symbol".to_owned(), json!(symbol));
    }
    map.extend(entry.attributes.clone());
    map
}

/// Whether the message names the symbol, by its id or by its own name.
fn mentions(message: &str, symbol: &str) -> bool {
    let id = symbol.split('#').next().unwrap_or(symbol);
    let name = id.rsplit(['/', '.', ':']).next().unwrap_or(id);
    contains_word(message, symbol) || (name.len() >= NAME_MIN && contains_word(message, name))
}

/// Whether `word` occurs in `text` with no identifier character on either
/// side, so `Foo` is not found in `FooBar`.
fn contains_word(text: &str, word: &str) -> bool {
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !identifier(c))
            && text[at + word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !identifier(c))
    })
}
