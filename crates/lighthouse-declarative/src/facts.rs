//! The code model as the values a rule's expressions see. Every field is
//! present for every value (an absent owner is the empty string), so an
//! expression never has to ask whether a field exists.

use std::path::Path;

use lighthouse_model::{
    Edge, Module, Node, Project, Symbol, SymbolKind, Target, TestCase, Visibility,
};
use serde_json::{Value, json};

pub(crate) fn visibility(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Internal => "internal",
    }
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub(crate) fn symbol(project: &Project, s: &Symbol) -> Value {
    let owner = s.owner.as_ref().and_then(|o| project.symbol(o));
    let lang = project
        .file(&s.file)
        .map_or_else(String::new, |f| f.lang.clone());
    json!({
        "id": s.id.as_str(),
        "name": s.name,
        "kind": s.kind.as_str(),
        "visibility": visibility(s.visibility),
        "owner": s.owner.as_ref().map_or("", |o| o.as_str()),
        "owner_kind": owner.map_or("", |o| o.kind.as_str()),
        "file": path(&s.file),
        "line": s.span.start.line,
        "end_line": s.span.end.line,
        "module": s.id.module(),
        "lang": lang,
        "documented": s.doc.is_some(),
        "test": project.in_test(&s.id),
        "generated": project.file(&s.file).is_some_and(|f| f.generated),
        "callers": project.callers(&s.id).len(),
        "callees": project.callees(&s.id).len(),
        "references": project.references(&s.id).len(),
        "members": project.members(&s.id).len(),
    })
}

/// A symbol with its function summary; the metrics are zero for a symbol
/// that has no body.
pub(crate) fn function(project: &Project, s: &Symbol) -> Option<Value> {
    let summary = project.function(&s.id)?;
    let mut value = symbol(project, s);
    let extra = json!({
        "statements": summary.statements,
        "top_level": summary.top_level,
        "params": summary.params,
        "returns": summary.returns,
        "max_nesting": summary.max_nesting,
        "tokens": summary.tokens,
        "branches": summary.flow.len(),
    });
    value.as_object_mut()?.extend(extra.as_object()?.clone());
    Some(value)
}

pub(crate) fn test(project: &Project, s: &Symbol, case: &TestCase) -> Value {
    let mut value = symbol(project, s);
    let targets: Vec<String> = case
        .targets
        .iter()
        .map(|t| match t {
            Target::Resolved(Node::Symbol(id)) => id.as_str().to_owned(),
            Target::Resolved(Node::Module(m)) | Target::Path(m) => m.clone(),
        })
        .collect();
    let extra = json!({
        "nesting": case.nesting,
        "style": match case.style {
            lighthouse_model::TestStyle::Table => "table",
            lighthouse_model::TestStyle::Scenario => "scenario",
        },
        "target_count": targets.len(),
        "targets": targets,
    });
    if let (Some(into), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        into.extend(extra.clone());
    }
    value
}

fn node(project: &Project, n: &Node) -> Value {
    match n {
        Node::Module(m) => json!({ "kind": "module", "id": m, "module": m, "name": m }),
        Node::Symbol(id) => {
            let name = project.symbol(id).map_or("", |s| s.name.as_str());
            json!({ "kind": "symbol", "id": id.as_str(), "module": id.module(), "name": name })
        }
    }
}

pub(crate) fn edge(project: &Project, e: &Edge) -> Value {
    let to = match &e.to {
        Target::Resolved(n) => node(project, n),
        Target::Path(p) => json!({ "kind": "unresolved", "id": p, "module": "", "name": p }),
    };
    json!({
        "kind": serde_json::to_value(e.kind).unwrap_or(Value::Null),
        "resolution": serde_json::to_value(e.resolution).unwrap_or(Value::Null),
        "from": node(project, &e.from),
        "to": to,
    })
}

pub(crate) fn module(project: &Project, m: &Module, files: usize, symbols: usize) -> Value {
    let _ = project;
    json!({
        "path": m.path,
        "name": m.name.clone().unwrap_or_else(|| m.path.clone()),
        "test_of": m.test_of.clone().unwrap_or_default(),
        "files": files,
        "symbols": symbols,
    })
}

pub(crate) fn file(project: &Project, f: &lighthouse_model::File, text: &str) -> Value {
    let symbols = project.symbols_in(&f.path).count();
    let functions = project
        .symbols_in(&f.path)
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .count();
    json!({
        "path": path(&f.path),
        "lang": f.lang,
        "test": f.test,
        "generated": f.generated,
        "lines": text.lines().count(),
        "symbols": symbols,
        "functions": functions,
    })
}
