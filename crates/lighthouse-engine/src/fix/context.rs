//! The localized context of a brief: what the code model already knows that
//! an agent would otherwise search the directory for. Localize first and hand
//! the repair exactly the code it needs (Agentless, Xia et al. 2024), as
//! signatures and not whole files (the repository map of Aider): the code of
//! each finding, the declarations of the project symbols it uses, the places
//! that refer to what it may change when the fix may reach other files, and
//! the imports of its file. The context has a budget; what does not fit is
//! counted. The same inputs make the same context, so that a run can be kept
//! under the digest of its brief. Nothing of a file the scratch copy of the
//! project leaves out, such as an environment file, is ever in it.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_model::{
    Diagnostic, Span, slashed,
    ucm::{EdgeKind, Node, Project, Symbol, Target},
};
use lighthouse_process::is_project_secret;
use lighthouse_spec::FixScope;

use super::{
    brief::{Code, Localized},
    model::choose_extent,
    rewrite::World,
};

/// How many characters make a token of the budget.
const CHARS_PER_TOKEN: usize = 4;
/// The longest line, in characters, of a declaration or a place.
const MAX_LINE: usize = 160;
/// How many lines a declaration takes at most.
const MAX_DECLARATION_LINES: usize = 3;
/// How many files the brief names as the ones the fix may have to change.
const MAX_FILES: usize = 40;

/// A declaration the code uses, given in the order of its rank while the
/// budget lasts: the nearer to the findings, the lower.
struct Item {
    rank: usize,
    text: String,
    /// The file of the symbol the text declares.
    file: String,
}

/// One place that refers to what the fix may change.
struct Place<'p> {
    file: &'p Path,
    line: u32,
    holder: Holder<'p>,
}

/// What holds a place: the declaration around a reference site, or the
/// symbol that stands for the place where the language reports no sites.
enum Holder<'p> {
    Symbol(&'p Symbol),
    Site(&'p Node),
}

/// The lines of the files of the world, read once each; none for a file the
/// scratch copy of the project leaves out.
struct Texts<'w, 'a> {
    world: &'w World<'a>,
    read: BTreeMap<PathBuf, Option<Vec<String>>>,
}

impl<'w, 'a> Texts<'w, 'a> {
    fn new(world: &'w World<'a>) -> Self {
        Self {
            world,
            read: BTreeMap::new(),
        }
    }

    fn lines(&mut self, path: &Path) -> Option<&[String]> {
        self.read
            .entry(path.to_owned())
            .or_insert_with(|| {
                if is_project_secret(path) {
                    return None;
                }
                let text = self.world.text(path)?;
                Some(text.lines().map(str::to_owned).collect())
            })
            .as_deref()
    }

    /// The line `number` of `path`, trimmed and cut.
    fn line(&mut self, path: &Path, number: u32) -> String {
        self.lines(path)
            .and_then(|lines| lines.get(number.max(1) as usize - 1))
            .map(|line| cut(line.trim()))
            .unwrap_or_default()
    }
}

/// The budget of a context, and what did not fit it.
struct Budget {
    left: usize,
    left_out: usize,
}

impl Budget {
    /// Whether `text` fits what is left; it takes that from it when it does,
    /// else it is counted as left out.
    fn take(&mut self, text: &str) -> bool {
        let cost = tokens(text);
        if cost > self.left {
            self.left_out += 1;
            return false;
        }
        self.left -= cost;
        true
    }
}

/// What a fix of scope `files` is told of the places that refer to what it
/// may change.
struct Reached {
    /// The places that fit the budget, as text.
    sites: Vec<String>,
    /// The files of all the places, at most [`MAX_FILES`].
    files: Vec<String>,
    /// The files the brief shows or names for them.
    shown: BTreeSet<String>,
}

/// The context of the findings of `file`, each with the extent an edit of it
/// may change when the scope holds it to one: the code of the extents and,
/// within `budget` tokens, what surrounds it.
pub(super) fn localize(
    world: &World<'_>,
    file: &Path,
    findings: &[(&Diagnostic, Option<Span>)],
    scope: FixScope,
    budget: usize,
) -> Localized {
    let project = &world.outcome.project;
    let mut texts = Texts::new(world);
    if texts.lines(file).is_none() {
        return Localized::default();
    }
    let language = project
        .file(file)
        .map(|f| f.lang.clone())
        .unwrap_or_default();
    let text = world.text(file).unwrap_or_default();
    let spans = extents(project, findings, &text);
    let code: Vec<Code> = spans
        .iter()
        .map(|span| code_of(file, &language, &text, *span))
        .collect();
    let anchors = anchors(project, findings);
    let mut budget = Budget {
        left: budget.saturating_sub(code.iter().map(|c| tokens(&c.text)).sum()),
        left_out: 0,
    };
    let mut localized = Localized {
        briefed: std::iter::once(slashed(file))
            .chain(code.iter().map(|c| c.path.clone()))
            .collect(),
        code,
        ..Localized::default()
    };
    let mut items = used(project, &anchors, file, &mut texts);
    items.sort_by_key(|item| item.rank);
    for item in items {
        if budget.take(&item.text) {
            localized.briefed.insert(item.file);
            localized.uses.push(item.text);
        }
    }
    if scope >= FixScope::References {
        let reached = reached(project, &anchors, file, &spans, &mut budget, &mut texts);
        localized.briefed.extend(reached.shown);
        localized.sites = reached.sites;
        localized.files = reached.files;
    }
    // The imports come last: they say least, so they take what is left.
    for name in imports_of(project, file) {
        if budget.take(&name) {
            localized.imports.push(name);
        }
    }
    localized.left_out = budget.left_out;
    localized
}

/// The places that refer to the `anchors`, as many as fit `budget`, and the
/// files that hold them all: the scope allows the change of every one of
/// them, not only of those that fit.
fn reached(
    project: &Project,
    anchors: &[&Symbol],
    file: &Path,
    spans: &[Span],
    budget: &mut Budget,
    texts: &mut Texts<'_, '_>,
) -> Reached {
    let places = places(project, anchors, file, spans);
    let mut sites = Vec::new();
    let mut shown = BTreeSet::new();
    for (at, place) in places.iter().enumerate() {
        if budget.left == 0 {
            budget.left_out += places.len() - at;
            break;
        }
        let text = rendered(place, project, texts);
        if budget.take(&text) {
            shown.extend(shown_in(place, project));
            sites.push(text);
        }
    }
    let files: BTreeSet<String> = places.iter().map(|p| slashed(p.file)).collect();
    budget.left_out += files.len().saturating_sub(MAX_FILES);
    let files: Vec<String> = files.into_iter().take(MAX_FILES).collect();
    shown.extend(files.iter().cloned());
    Reached {
        sites,
        files,
        shown,
    }
}

/// The tokens the budget counts for `text`.
fn tokens(text: &str) -> usize {
    text.len().div_ceil(CHARS_PER_TOKEN)
}

/// The spans of the extents of the findings, each once, in file order, and
/// none inside another: the one the scope gives, else what [`choose_extent`]
/// finds, else none.
fn extents(project: &Project, findings: &[(&Diagnostic, Option<Span>)], text: &str) -> Vec<Span> {
    let mut found: Vec<Span> = findings
        .iter()
        .filter_map(|(finding, span)| span.or_else(|| choose_extent(project, finding, text).ok()))
        .collect();
    // The one that holds the others comes first.
    found.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end)));
    let mut spans: Vec<Span> = Vec::new();
    for span in found {
        if !spans
            .iter()
            .any(|s| s.start <= span.start && span.end <= s.end)
        {
            spans.push(span);
        }
    }
    spans
}

/// The lines of `span` in `text`.
fn code_of(file: &Path, language: &str, text: &str, span: Span) -> Code {
    let first = span.start.line.max(1);
    let count = (span.end.line.max(first) - first + 1) as usize;
    let lines: Vec<&str> = text.lines().skip(first as usize - 1).take(count).collect();
    Code {
        path: slashed(file),
        language: language.to_owned(),
        first,
        text: lines.join("\n"),
    }
}

/// The symbols the findings are about, each once, by id.
fn anchors<'p>(project: &'p Project, findings: &[(&Diagnostic, Option<Span>)]) -> Vec<&'p Symbol> {
    let mut found: BTreeMap<&str, &Symbol> = BTreeMap::new();
    for (finding, _) in findings {
        if let Some(symbol) = project.anchor(finding) {
            found.insert(symbol.id.as_str(), symbol);
        }
    }
    found.into_values().collect()
}

/// The symbols of the project the `anchors` use, as declarations, those of
/// the file first.
fn used(
    project: &Project,
    anchors: &[&Symbol],
    file: &Path,
    texts: &mut Texts<'_, '_>,
) -> Vec<Item> {
    let own: BTreeSet<&str> = anchors.iter().map(|s| s.id.as_str()).collect();
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for anchor in anchors {
        for id in project.uses(&anchor.id) {
            let Some(symbol) = project.symbol(id) else {
                continue;
            };
            if own.contains(id.as_str()) || !seen.insert(id.as_str()) {
                continue;
            }
            if is_project_secret(&symbol.file) {
                continue;
            }
            items.push(Item {
                rank: if symbol.file == file { 1 } else { 2 },
                text: declared(symbol, texts),
                file: slashed(&symbol.file),
            });
        }
    }
    items
}

/// What `file` imports, one name each, in order.
fn imports_of(project: &Project, file: &Path) -> Vec<String> {
    let names: BTreeSet<&str> = project
        .edges
        .iter()
        .enumerate()
        .filter(|(at, edge)| edge.kind == EdgeKind::Imports && project.origin(*at) == Some(file))
        .map(|(_, edge)| match &edge.to {
            Target::Resolved(Node::Module(name)) | Target::Path(name) => name.as_str(),
            Target::Resolved(Node::Symbol(id)) => id.as_str(),
        })
        .collect();
    names.into_iter().map(str::to_owned).collect()
}

/// The places that refer to the `anchors`, outside the `spans` of the code
/// given, nearest the file first. Where the language reports no reference
/// sites, the symbols that call or refer to them stand for the places. None
/// is built into text here: that is done only while the budget lasts.
fn places<'p>(
    project: &'p Project,
    anchors: &[&Symbol],
    file: &Path,
    spans: &[Span],
) -> Vec<Place<'p>> {
    let inside = |at: &Path, line: u32| {
        at == file
            && spans
                .iter()
                .any(|s| s.start.line <= line && line <= s.end.line)
    };
    let mut found: BTreeMap<(bool, &Path, u32, String), Place<'p>> = BTreeMap::new();
    for anchor in anchors {
        let sites = project.sites(&anchor.id);
        for site in sites {
            let line = site.span.start.line;
            if inside(&site.file, line) || is_project_secret(&site.file) {
                continue;
            }
            let held = match &site.from {
                Node::Symbol(id) => id.as_str().to_owned(),
                Node::Module(name) => name.clone(),
            };
            found.insert(
                (site.file != file, site.file.as_path(), line, held),
                Place {
                    file: &site.file,
                    line,
                    holder: Holder::Site(&site.from),
                },
            );
        }
        if sites.is_empty() {
            for id in project
                .callers(&anchor.id)
                .iter()
                .chain(project.references(&anchor.id))
            {
                let Some(symbol) = project.symbol(id) else {
                    continue;
                };
                if is_project_secret(&symbol.file) {
                    continue;
                }
                let line = symbol.span.start.line;
                found.insert(
                    (
                        symbol.file != file,
                        symbol.file.as_path(),
                        line,
                        id.as_str().to_owned(),
                    ),
                    Place {
                        file: &symbol.file,
                        line,
                        holder: Holder::Symbol(symbol),
                    },
                );
            }
        }
    }
    found.into_values().collect()
}

/// The files the text of a place shows: its own and that of the declaration
/// around it.
fn shown_in(place: &Place<'_>, project: &Project) -> Vec<String> {
    let holder = match place.holder {
        Holder::Symbol(symbol) => Some(symbol),
        Holder::Site(Node::Symbol(id)) => project.symbol(id),
        Holder::Site(Node::Module(_)) => None,
    };
    std::iter::once(slashed(place.file))
        .chain(holder.map(|symbol| slashed(&symbol.file)))
        .collect()
}

/// The text of a place: where it is, its line and what holds it.
fn rendered(place: &Place<'_>, project: &Project, texts: &mut Texts<'_, '_>) -> String {
    let at = format!("{}:{}", slashed(place.file), place.line);
    match place.holder {
        Holder::Symbol(symbol) => format!("{at}  {}", declared(symbol, texts)),
        Holder::Site(Node::Symbol(id)) => {
            let said = texts.line(place.file, place.line);
            let holder = project
                .symbol(id)
                .map_or_else(String::new, |s| declared(s, texts));
            format!("{at}  {said}  in {holder}")
        }
        Holder::Site(Node::Module(name)) => {
            let said = texts.line(place.file, place.line);
            format!("{at}  {said}  in module {name}")
        }
    }
}

/// The declaration of `symbol`: its first lines up to the body, then where it
/// is and the first line of its doc.
fn declared(symbol: &Symbol, texts: &mut Texts<'_, '_>) -> String {
    let mut declaration = String::new();
    let start = symbol.span.start.line.max(1) as usize - 1;
    if let Some(lines) = texts.lines(&symbol.file) {
        for line in lines.iter().skip(start).take(MAX_DECLARATION_LINES) {
            let line = line.trim();
            if let Some(body) = line.find('{') {
                join(&mut declaration, line[..body].trim_end());
                break;
            }
            join(&mut declaration, line);
            if line.ends_with(';') || line.ends_with(':') {
                break;
            }
        }
    }
    let mut text = format!(
        "{}  ({}:{})",
        cut(&declaration),
        slashed(&symbol.file),
        symbol.span.start.line
    );
    if let Some(doc) = symbol
        .doc
        .as_deref()
        .and_then(|doc| doc.lines().map(str::trim).find(|line| !line.is_empty()))
    {
        text.push_str(" - ");
        text.push_str(&cut(doc));
    }
    text
}

fn join(text: &mut String, part: &str) {
    if !text.is_empty() && !part.is_empty() {
        text.push(' ');
    }
    text.push_str(part);
}

/// `line` cut to [`MAX_LINE`] characters.
fn cut(line: &str) -> String {
    line.chars().take(MAX_LINE).collect()
}

#[cfg(test)]
mod tests;
