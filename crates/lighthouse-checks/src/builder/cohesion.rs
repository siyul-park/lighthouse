//! The `types` of a module: its types and interfaces with the size of their
//! API and the cohesion of their methods (LCOM4: the connected components of
//! the methods, joined by a field they share or a call between them).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use lighthouse_model::{EdgeKind, Node, Project, Symbol, SymbolId, SymbolKind, Target, Visibility};
use serde_json::{Value, json};

use super::Builder;

/// The types of every module, built by the first check that asks and shared
/// by the others.
#[derive(Default)]
pub(crate) struct Types(OnceLock<BTreeMap<String, Vec<Value>>>);

impl Builder<'_> {
    /// The non-test, non-generated types and interfaces of a module.
    pub(super) fn module_types(&self, module: &str) -> Value {
        let types = self.types.0.get_or_init(|| index(self.project));
        json!(types.get(module).map_or(&[][..], Vec::as_slice))
    }
}

fn index(project: &Project) -> BTreeMap<String, Vec<Value>> {
    let mut found: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let types = project
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, SymbolKind::Type | SymbolKind::Interface))
        .filter(|s| !project.in_test(&s.id))
        .filter(|s| project.file(&s.file).is_some_and(|f| !f.generated));
    for ty in types {
        found
            .entry(ty.id.module().to_owned())
            .or_default()
            .push(entry(project, ty));
    }
    found
}

fn entry(project: &Project, ty: &Symbol) -> Value {
    let methods: Vec<&Symbol> = project
        .members(&ty.id)
        .iter()
        .filter_map(|id| project.symbol(id))
        .filter(|s| s.kind == SymbolKind::Method)
        .collect();
    let own: Vec<&&Symbol> = methods
        .iter()
        .filter(|m| !project.function(&m.id).is_some_and(|f| f.implementation))
        .collect();
    let groups = groups(project, ty, &methods);
    json!({
        "id": ty.id.as_str(),
        "name": ty.name,
        "file": ty.file.to_string_lossy(),
        "line": ty.span.start.line,
        "methods": own.len(),
        "public_methods": own.iter().filter(|m| m.visibility == Visibility::Public).count(),
        "lcom4": groups.len(),
        "groups": groups,
    })
}

/// The names of the methods of each connected component, the largest
/// component first. The methods of an interface declare no bodies and share
/// nothing: they are one group.
fn groups(project: &Project, ty: &Symbol, methods: &[&Symbol]) -> Vec<Vec<String>> {
    if methods.is_empty() {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..methods.len()).collect();
    if ty.kind == SymbolKind::Type {
        join_methods(project, ty, methods, &mut parent);
    } else {
        parent.fill(0);
    }
    let mut components: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (at, method) in methods.iter().enumerate() {
        let root = root(&mut parent, at);
        components
            .entry(root)
            .or_default()
            .push(method.name.clone());
    }
    let mut groups: Vec<Vec<String>> = components.into_values().collect();
    for names in &mut groups {
        names.sort();
    }
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    groups
}

/// Joins the methods that call each other or use the same field of `ty`.
fn join_methods(project: &Project, ty: &Symbol, methods: &[&Symbol], parent: &mut [usize]) {
    let at: BTreeMap<&SymbolId, usize> = methods
        .iter()
        .enumerate()
        .map(|(i, m)| (&m.id, i))
        .collect();
    let fields: BTreeSet<&SymbolId> = project
        .members(&ty.id)
        .iter()
        .filter(|id| {
            project
                .symbol(id)
                .is_some_and(|s| s.kind == SymbolKind::Field)
        })
        .collect();
    let mut user: BTreeMap<&SymbolId, usize> = BTreeMap::new();
    for (i, method) in methods.iter().enumerate() {
        for edge in project.edges_from(&method.id) {
            let Target::Resolved(Node::Symbol(to)) = &edge.to else {
                continue;
            };
            let other = if let Some(&j) = at.get(to) {
                Some(j)
            } else if edge.kind == EdgeKind::References && fields.contains(to) {
                Some(*user.entry(to).or_insert(i))
            } else {
                None
            };
            if let Some(j) = other {
                let (a, b) = (root(parent, i), root(parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
}

fn root(parent: &mut [usize], mut at: usize) -> usize {
    while parent[at] != at {
        parent[at] = parent[parent[at]];
        at = parent[at];
    }
    at
}
