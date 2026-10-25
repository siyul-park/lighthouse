//! The code model as the values a rule's expressions see. Every field is
//! present for every value (an absent owner is the empty string), so an
//! expression never has to ask whether a field exists.

use std::path::Path;

use lighthouse_model::{
    Document, Position,
    ucm::{
        Edge, Module, Node, Project, Symbol, SymbolKind, SymbolRole, Target, TestCase, Visibility,
    },
};

use crate::{
    layout::{is_declaration, owner_key},
    modules::Declared,
    text,
};
use serde_json::{Value, json};

const POS_LINE: u64 = 1 << 20;

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

/// The fields that identify a declaration, for the nodes of a list.
pub(crate) fn node(project: &Project, s: &Symbol) -> Value {
    let owner = s.owner.as_ref().and_then(|o| project.symbol(o));
    let file = project.file(&s.file);
    json!({
        "id": s.id.as_str(),
        "name": s.name,
        "kind": s.kind.as_str(),
        "visibility": visibility(s.visibility),
        "owner_kind": owner.map_or("", |o| o.kind.as_str()),
        "file": path(&s.file),
        "line": s.span.start.line,
        "col": s.span.start.col,
        "pos": pos(s.span.start),
        "end_line": s.span.end.line,
        "module": s.id.module(),
        "test_role": s.role.map_or("", role),
        "documented": s.doc.is_some(),
        "test": project.in_test(&s.id),
        "file_test": file.is_some_and(|f| f.test),
        "generated": file.is_some_and(|f| f.generated),
        "owner_key": owner_key(s).unwrap_or_default(),
        "declaration": is_declaration(project, s),
        // The method implements one a trait or interface declares elsewhere:
        // its placement and ownership are that declaration's, as the provider says.
        "implementation": project.function(&s.id).is_some_and(|f| f.implementation),
    })
}

pub(crate) fn symbol(project: &Project, s: &Symbol) -> Value {
    let lang = project
        .file(&s.file)
        .map_or_else(String::new, |file| file.analysis_language().to_owned());
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
    extended(
        node(project, s),
        json!({
        "lang": lang,
        "doc": s.doc.clone().unwrap_or_default(),
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
    )
}

/// A symbol with its function summary; the metrics are zero for a symbol
/// that has no body.
pub(crate) fn function(project: &Project, s: &Symbol) -> Option<Value> {
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

pub(crate) fn test(project: &Project, s: &Symbol, case: &TestCase) -> Value {
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

pub(crate) fn edge(project: &Project, e: &Edge) -> Value {
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

pub(crate) fn file(project: &Project, f: &lighthouse_model::ucm::File, text: &str) -> Value {
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

fn graph_node(project: &Project, n: &Node) -> Value {
    match n {
        Node::Module(m) => json!({ "kind": "module", "id": m, "module": m, "name": m }),
        Node::Symbol(id) => {
            let name = project.symbol(id).map_or("", |s| s.name.as_str());
            json!({ "kind": "symbol", "id": id.as_str(), "module": id.module(), "name": name })
        }
    }
}
