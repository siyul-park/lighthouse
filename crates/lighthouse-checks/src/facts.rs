//! The code model as the values a rule's expressions see. Every field is
//! present for every value (an absent owner is the empty string), so an
//! expression never has to ask whether a field exists.

use std::path::Path;

use lighthouse_model::{
    Document, Position, Span,
    ucm::{
        Edge, Model, Module, Node, Symbol, SymbolId, SymbolKind, SymbolRole, Target, TestCase,
        Visibility,
    },
};

use crate::{
    layout::{is_declaration, owner_key},
    modules::Declared,
    text,
};
use serde_json::{Value, json};

const POS_LINE: u64 = 1 << 20;

/// The span of a declaration whose place the model does not hold.
const UNPLACED: Span = Span {
    start: Position { line: 1, col: 1 },
    end: Position { line: 1, col: 1 },
};

pub(crate) fn visibility(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Internal => "internal",
    }
}

/// A position as one number that orders like (line, column).
pub(crate) fn pos(p: Position) -> u64 {
    u64::from(p.line) * POS_LINE + u64::from(p.col)
}

/// Where a declaration is: the span of its place, which moves with any edit
/// above it.
pub(crate) fn span_of(project: &dyn Model, id: &SymbolId) -> Span {
    project.place(id).map_or(UNPLACED, |place| place.span)
}

/// The whole declaration, from its first doc comment or attribute to its last
/// token; its span where the provider does not report the extent.
pub(crate) fn extent_of(project: &dyn Model, id: &SymbolId) -> Span {
    project.place(id).map_or(UNPLACED, |place| place.whole())
}

/// The fields that identify a declaration, for the nodes of a list.
pub(crate) fn node(project: &dyn Model, s: &Symbol) -> Value {
    shaped(project, s, true)
}

/// The stem of a file: what its provider states, else its name without the
/// extension. A provider leaves out of it what the language reads as no part
/// of the name, such as Go's build constraint suffixes.
pub(crate) fn file_stem(project: &dyn Model, file: &Path) -> String {
    project
        .file(file)
        .and_then(|f| f.facts.get("stem").cloned())
        .or_else(|| {
            file.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

pub(crate) fn symbol(project: &dyn Model, s: &Symbol) -> Value {
    symbol_with(project, s, true)
}

/// A symbol as [`symbol`] has it, with the fields that are where it is and
/// what it says (`doc` too) only when `placed`.
pub(crate) fn symbol_with(project: &dyn Model, s: &Symbol, placed: bool) -> Value {
    let lang = project
        .language_of(s)
        .map_or_else(String::new, str::to_owned);
    let module = project.module(s.id.module());
    let last = module.map_or_else(
        || {
            s.id.module()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        },
        |m| m.path.rsplit('/').next().unwrap_or(&m.path).to_owned(),
    );
    let spelled = module
        .and_then(|m| m.name.clone())
        .unwrap_or_else(|| last.clone());
    let doc = placed
        .then(|| project.place(&s.id).and_then(|p| p.doc.clone()))
        .flatten()
        .unwrap_or_default();
    let mut value = extended(
        shaped(project, s, placed),
        json!({
        "lang": lang,
        "name_key": text::key(&s.name),
        "module_forms": if module.is_some() {
            json!([
                { "name": spelled, "key": text::key(&spelled) },
                { "name": last, "key": text::key(&last) },
            ])
        } else {
            json!([])
        },
        }),
    );
    if placed && let Some(map) = value.as_object_mut() {
        map.insert("doc".to_owned(), json!(doc));
    }
    value
}

/// A symbol with its function summary; the metrics are zero for a symbol
/// that has no body.
pub(crate) fn function(project: &dyn Model, s: &Symbol) -> Option<Value> {
    let summary = project.function(&s.id)?;
    Some(extended(
        symbol(project, s),
        json!({
        "statements": summary.statements,
        "top_level": summary.top_level,
        "max_nesting": summary.max_nesting,
        "tokens": summary.tokens,
        "branches": summary.flow.len(),
        "manual_assertions": summary.manual_assertions,
        }),
    ))
}

pub(crate) fn test(project: &dyn Model, s: &Symbol, case: &TestCase) -> Value {
    let targets: Vec<String> = case
        .targets
        .iter()
        .map(|t| match t {
            Target::Resolved(Node::Symbol(id)) => id.as_str().to_owned(),
            Target::Resolved(Node::Module(m)) | Target::Path(m) => m.clone(),
        })
        .collect();
    extended(
        symbol(project, s),
        json!({
        "nesting": case.nesting,
        "style": match case.style {
            lighthouse_model::ucm::TestStyle::Table => "table",
            lighthouse_model::ucm::TestStyle::Scenario => "scenario",
        },
        "target_count": targets.len(),
        "targets": targets,
        }),
    )
}

pub(crate) fn edge(project: &dyn Model, e: &Edge) -> Value {
    let to = match &e.to {
        Target::Resolved(n) => graph_node(project, n),
        Target::Path(p) => json!({ "kind": "unresolved", "id": p, "module": "", "name": p }),
    };
    json!({
        "kind": serde_json::to_value(e.kind).unwrap_or(Value::Null),
        "resolution": serde_json::to_value(e.resolution).unwrap_or(Value::Null),
        "from": graph_node(project, &e.from),
        "to": to,
    })
}

/// A document of the project, such as a decision: its name, the pack its name
/// puts it in, and where it is written.
pub(crate) fn document(d: &Document) -> Value {
    let pack = d.name.split_once('/').map_or(d.name.as_str(), |(p, _)| p);
    json!({
        "name": d.name,
        "kind": d.kind,
        "pack": pack,
        "labels": d.labels,
        "uid": d.uid.clone().unwrap_or_default(),
        "file": path(&d.file),
        "line": d.at.line,
    })
}

pub(crate) fn module(m: &Module, declared: &Declared) -> Value {
    json!({
        "path": m.path,
        "name": m.name.clone().unwrap_or_else(|| m.path.clone()),
        "lang": declared.lang,
        "test_of": m.test_of.clone().unwrap_or_default(),
        "files": declared.files.len(),
        "lines": declared.lines,
        "dependents": declared.dependents,
        "declares": declared.declares,
        "children": declared.children,
    })
}

pub(crate) fn file(project: &dyn Model, f: &lighthouse_model::ucm::File, text: &str) -> Value {
    let functions = project
        .symbols_in(&f.path)
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .count();
    json!({
        "path": path(&f.path),
        "lang": f.analysis_language(),
        "host_lang": f.lang,
        "test": f.test,
        "generated": f.generated,
        "lines": text.lines().count(),
        "functions": functions,
    })
}

/// A node with its place fields (`line`, `col`, `pos`, `end_line` and
/// `documented`) only when `placed`: without them it is the declaration's
/// shape, which a body edit leaves as it was.
fn shaped(project: &dyn Model, s: &Symbol, placed: bool) -> Value {
    let owner = s.owner.as_ref().and_then(|o| project.symbol(o));
    let file = project.file(&s.file);
    let mut value = json!({
        "id": s.id.as_str(),
        "name": s.name,
        "name_key": text::key(&s.name),
        "kind": s.kind.as_str(),
        "visibility": visibility(s.visibility),
        "owner_kind": owner.map_or("", |o| o.kind.as_str()),
        "owner_visibility": owner.map_or("", |o| visibility(o.visibility)),
        "file": path(&s.file),
        // The stem of the declaring file: its name without the extension and
        // what its language ignores in a name (Go's build constraint suffixes).
        "file_stem": file_stem(project, &s.file),
        "module": s.id.module(),
        "test_role": s.role.map_or("", role),
        "test": project.in_test(&s.id),
        "file_test": file.is_some_and(|f| f.test),
        "generated": file.is_some_and(|f| f.generated),
        "owner_key": owner_key(s).unwrap_or_default(),
        "declaration": is_declaration(project, s),
        // The method implements one a trait or interface declares elsewhere:
        // its placement and ownership are that declaration's, as the provider says.
        "implementation": s.implementation,
    });
    if placed && let Some(map) = value.as_object_mut() {
        let place = project.place(&s.id);
        let span = place.map_or(UNPLACED, |p| p.span);
        map.insert("line".to_owned(), json!(span.start.line));
        map.insert("col".to_owned(), json!(span.start.col));
        map.insert("pos".to_owned(), json!(pos(span.start)));
        map.insert("end_line".to_owned(), json!(span.end.line));
        map.insert(
            "documented".to_owned(),
            json!(place.is_some_and(|p| p.doc.is_some())),
        );
    }
    value
}

/// `value` with the fields of the object `extra` added.
fn extended(mut value: Value, extra: Value) -> Value {
    if let (Some(into), Value::Object(extra)) = (value.as_object_mut(), extra) {
        into.extend(extra);
    }
    value
}

fn role(role: SymbolRole) -> &'static str {
    match role {
        SymbolRole::TestHelper => "test-helper",
        SymbolRole::Fixture => "fixture",
    }
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn graph_node(project: &dyn Model, n: &Node) -> Value {
    match n {
        Node::Module(m) => json!({ "kind": "module", "id": m, "module": m, "name": m }),
        Node::Symbol(id) => {
            let name = project.symbol(id).map_or("", |s| s.name.as_str());
            json!({ "kind": "symbol", "id": id.as_str(), "module": id.module(), "name": name })
        }
    }
}
