use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    path::Path,
};

use lighthouse_model::{Safety, Severity, Verdict};
use lighthouse_spec::{Catalog, Decision, FixKind};
use serde_json::{Map, Value, json};

use crate::{
    evidence::{bounded, evidence, pairs},
    expected::{Subject, expected, one_line},
    fix::{ProposedFix, Shown},
};

/// Shortest fingerprint prefix shown, as in git; longer when the findings of
/// the run share more.
const PREFIX_MIN: usize = 7;
/// Longest a symbol's own name may be to count as mentioned by a message.
const NAME_MIN: usize = 3;
/// How a verdict is recorded, said once per report.
const RESOLVE: &str = "review_resolve {fingerprint, verdict, reason}";

/// One finding as the compact shape sees it, wherever it comes from: a fresh
/// diagnostic or a remembered finding.
#[derive(Debug, Clone)]
pub struct Entry<'a> {
    pub rule: String,
    pub severity: Severity,
    /// The severity the decision authored; shown only when it differs.
    pub authored: Severity,
    /// Whether the finding asks for a verdict.
    pub review: bool,
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub symbol: Option<String>,
    pub evidence: Value,
    /// More evidence-like facts, shared by a group when they agree.
    pub attributes: Map<String, Value>,
    /// Why the finding is reported although a verdict was recorded on it.
    pub note: Option<String>,
    pub fingerprint: String,
    /// Subject facts: `language`, `kind` and `visibility` pick the expected
    /// structure.
    pub facts: Option<Value>,
    /// The fix proposed for the finding, when one was computed.
    pub fix: Option<&'a ProposedFix>,
}

/// What shapes the groups.
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupOptions<'a> {
    /// The decisions findings cite.
    pub catalog: Option<&'a Catalog>,
    /// Keep at most this many findings.
    pub limit: Option<usize>,
    /// The groups are read through MCP, so fixing is the `fix` tool and not
    /// the command line.
    pub mcp: bool,
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
    rule: String,
    severity: Severity,
    authored: Option<Severity>,
    /// Some finding of the group asks for a verdict; when only some do,
    /// their instances say which.
    review: bool,
    requirement: Option<String>,
    /// The expected structure by language, once for all findings of it.
    expected: Vec<(Option<String>, String)>,
    /// How to apply the fix of the decision, when it has one.
    apply: Option<String>,
    /// The evidence every finding of the group agrees on.
    evidence: Map<String, Value>,
    files: BTreeMap<String, Vec<Instance>>,
}

/// One finding: where, what, which, and what sets it apart from the group.
#[derive(Debug, Clone)]
struct Instance {
    full: String,
    line: u32,
    col: u32,
    message: String,
    fingerprint: String,
    evidence: Map<String, Value>,
    note: Option<String>,
    fix: Option<Shown>,
    /// Asks for a verdict; set only in a mixed group.
    review: bool,
}

impl Grouped {
    /// The groups of `entries`: errors first, then the larger ones. At most
    /// `limit` findings are kept, in that order; what does not fit is counted.
    /// Fingerprint prefixes are unique among all `entries`, shown or not.
    pub fn of(entries: Vec<Entry<'_>>, options: &GroupOptions) -> Self {
        let limit = options.limit;
        let widths = prefix_widths(&entries);
        let mut buckets: BTreeMap<(Severity, &str), Vec<(&Entry<'_>, usize)>> = BTreeMap::new();
        for (entry, width) in entries.iter().zip(widths) {
            buckets
                .entry((entry.severity, entry.rule.as_str()))
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

    /// The full fingerprints of the findings kept.
    pub fn fingerprints(&self) -> BTreeSet<&str> {
        self.groups
            .iter()
            .flat_map(|g| g.files.values().flatten())
            .map(|i| i.full.as_str())
            .collect()
    }

    /// Whether any shown finding asks for a verdict.
    pub fn asks_review(&self) -> bool {
        self.groups.iter().any(|g| g.review)
    }

    /// `groups`, then when they apply `omitted` (`groups` and `findings` left
    /// out) and, once for all groups, the `resolve` hint and the `reasons`
    /// table.
    pub fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        let groups = self.groups.iter().map(Group::to_json).collect();
        fields.insert("groups".to_owned(), Value::Array(groups));
        if self.omitted_findings > 0 {
            fields.insert(
                "omitted".to_owned(),
                json!({ "groups": self.omitted_groups, "findings": self.omitted_findings }),
            );
        }
        if self.asks_review() {
            fields.insert("resolve".to_owned(), json!(RESOLVE));
            fields.insert("reasons".to_owned(), reasons());
        }
        fields
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
    fn build(mut rows: Vec<(&Entry<'_>, usize)>, take: usize, options: &GroupOptions) -> Self {
        rows.sort_by(|(a, _), (b, _)| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        rows.truncate(take);
        let first = rows[0].0;
        let review = rows.iter().any(|(entry, _)| entry.review);
        let mixed = review && rows.iter().any(|(entry, _)| !entry.review);
        let decision = options.catalog.and_then(|c| c.decision(&first.rule));
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
                let subject = Subject::of(entry.facts.as_ref());
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
                full: entry.fingerprint.clone(),
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
                fix: entry.fix.map(|fix| Shown::of(fix, &entry.path)),
                review: mixed && entry.review,
            });
        }
        Self {
            rule: first.rule.clone(),
            severity: first.severity,
            authored: (first.authored != first.severity).then_some(first.authored),
            review,
            requirement: decision.map(|d| one_line(&d.requirement)),
            expected: expecteds,
            apply: decision.and_then(|d| apply_line(d, options.mcp)),
            evidence: shared,
            files,
        }
    }

    fn to_json(&self) -> Value {
        let mut group = Map::new();
        group.insert("rule".to_owned(), json!(self.rule));
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
        if let Some(apply) = &self.apply {
            group.insert("apply".to_owned(), json!(apply));
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
        let _ = write!(out, "{} {}", self.rule, self.severity);
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
        if let Some(apply) = &self.apply {
            let _ = writeln!(out, "  apply: {apply}");
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
        if let Some(fix) = &self.fix {
            varying.insert("fix".to_owned(), fix.json());
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
        if let Some(fix) = &self.fix {
            for line in fix.text("    ") {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
}

/// The reasons each verdict accepts.
pub(crate) fn reasons() -> Value {
    let verdicts: BTreeMap<&str, Vec<&str>> =
        [Verdict::Confirmed, Verdict::Rejected, Verdict::Deferred]
            .into_iter()
            .map(|v| (v.as_str(), v.reasons().iter().map(|r| r.as_str()).collect()))
            .collect();
    json!(verdicts)
}

pub(crate) fn reasons_line() -> String {
    [Verdict::Confirmed, Verdict::Rejected, Verdict::Deferred]
        .into_iter()
        .map(|v| {
            let reasons: Vec<&str> = v.reasons().iter().map(|r| r.as_str()).collect();
            format!("{v}={}", reasons.join("|"))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// How the fix of a decision is applied for all its findings: `check --fix`
/// on the command line, the `fix` tool through MCP, with unsafe fixes when the
/// decision's fix is only suggested or its rule is not mechanical. None for a
/// decision without a fix that can run.
fn apply_line(decision: &Decision, mcp: bool) -> Option<String> {
    let fix = decision.fix.as_ref()?;
    if matches!(fix.kind, FixKind::Rpc { .. }) {
        return None;
    }
    let unsafe_fixes =
        fix.safety == Safety::Suggested || decision.severity() != Some(Severity::Error);
    let rule = decision.id();
    Some(match (mcp, unsafe_fixes) {
        (false, false) => format!("lighthouse check --fix --rules {rule}"),
        (false, true) => format!("lighthouse check --fix --unsafe-fixes --rules {rule}"),
        (true, false) => format!("fix {{\"rules\":[\"{rule}\"]}}"),
        (true, true) => format!("fix {{\"rules\":[\"{rule}\"],\"unsafeFixes\":true}}"),
    })
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

/// For each entry, how many characters of its fingerprint tell it apart from
/// every other entry, at least [`PREFIX_MIN`].
fn prefix_widths(entries: &[Entry]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| entries[a].fingerprint.cmp(&entries[b].fingerprint));
    let mut widths = vec![PREFIX_MIN; entries.len()];
    for pair in order.windows(2) {
        let (a, b) = (&entries[pair[0]].fingerprint, &entries[pair[1]].fingerprint);
        let shared = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
        for &i in pair {
            widths[i] = widths[i].max(shared + 1);
        }
    }
    widths
}
