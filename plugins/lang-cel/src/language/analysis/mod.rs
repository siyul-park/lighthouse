//! The references and calls of one CEL expression, as edges of the code model
//! from the symbol that holds the expression, and the summary of its size.

use std::collections::{BTreeMap, HashSet};

use cel::common::ast::{CallExpr, ComprehensionExpr, EntryExpr, Expr, IdedEntryExpr, IdedExpr};
use lighthouse_model::{
    Span,
    ucm::{
        Edge, EdgeKind, FunctionSummary, Node, Resolution, Symbol, SymbolId, SymbolKind, Target,
    },
};
use lighthouse_plugin::CelLibraryManifest;

use super::source::{MappedText, Parameter, VariableDecl};

mod metrics;
mod names;
mod spans;

use metrics::{Metrics, token_count};
pub(super) use names::{external_module, function_catalog};
use names::{external_symbol, library_target, standard_target};
use spans::{call_span, expr_span, field_span};

/// Root values supplied by check, fix and recipe contexts, not by the host file.
const CONTEXT_VARIABLES: [&str; 16] = [
    "symbol",
    "func",
    "edge",
    "module",
    "file",
    "test",
    "comment",
    "annotation",
    "event",
    "decision",
    "node",
    "options",
    "finding",
    "line",
    "tools",
    "item",
];

/// The document of a pack: the variables a decision of the pack may use
/// without declaring them.
const PACK: &str = "Pack";
const DECISION: &str = "Decision";

/// The shared variables of the packs by name: the document that declares each,
/// its symbol and whether it takes parameters.
pub(super) type PackVariables = BTreeMap<String, Vec<(String, SymbolId, bool)>>;

/// What the expressions of one file resolve their names against, and the
/// symbols of functions outside the project they found on the way.
pub(super) struct Analysis<'a> {
    variables: &'a [VariableDecl],
    packs: &'a PackVariables,
    libraries: &'a [CelLibraryManifest],
    external: &'a mut BTreeMap<SymbolId, Symbol>,
}

/// One expression to analyze: its program and text, the symbol that holds it,
/// the document it belongs to and the parameters it may name.
struct Expression<'e> {
    program: &'e cel::Program,
    mapped: &'e MappedText,
    from: &'e SymbolId,
    doc_id: &'e str,
    params: &'e [Parameter],
}

/// The symbol a call resolves to.
struct Resolved {
    id: SymbolId,
    resolution: Resolution,
    /// The call is made on the namespace of its function: its target is no
    /// value to walk.
    namespaced: bool,
}

impl<'a> Analysis<'a> {
    pub(super) fn new(
        variables: &'a [VariableDecl],
        packs: &'a PackVariables,
        libraries: &'a [CelLibraryManifest],
        external: &'a mut BTreeMap<SymbolId, Symbol>,
    ) -> Self {
        Self {
            variables,
            packs,
            libraries,
            external,
        }
    }

    pub(super) fn run(
        &mut self,
        program: &cel::Program,
        mapped: &MappedText,
        from: &SymbolId,
        doc_id: &str,
        params: &[Parameter],
    ) -> (FunctionSummary, Vec<Edge>) {
        let mut summary = Metrics::of(program.expression()).summary(
            from.clone(),
            params.len() as u32,
            token_count(&mapped.text),
            self.forwards_to(program, from, doc_id),
        );
        summary.signature.results[0].text = metrics::result_type(program.expression()).to_owned();
        let at = Expression {
            program,
            mapped,
            from,
            doc_id,
            params,
        };
        let mut edges = Vec::new();
        self.walk(&at, program.expression(), &HashSet::new(), &mut edges);
        (summary, edges)
    }

    /// The variable a variable of no parameters only repeats, if it does.
    fn forwards_to(&self, program: &cel::Program, from: &SymbolId, doc_id: &str) -> Option<Target> {
        let variable = self
            .variables
            .iter()
            .find(|variable| variable.id == *from && variable.doc_id == doc_id)?;
        if !variable.params.is_empty() {
            return None;
        }
        let Expr::Ident(name) = &program.expression().expr else {
            return None;
        };
        let target = self
            .declared(name, doc_id)
            .or_else(|| self.pack_variable(name, doc_id).map(|(id, _)| id.clone()))?;
        (target != *from).then(|| Target::Path(target.as_str().to_owned()))
    }

    /// The variable `name` of the document, as the document declares it.
    fn declared(&self, name: &str, doc_id: &str) -> Option<SymbolId> {
        self.variables
            .iter()
            .find(|variable| variable.doc_id == doc_id && variable.name == name)
            .map(|variable| variable.id.clone())
    }

    /// The shared variable `name` of the pack that the decision `doc_id`
    /// belongs to, and whether it takes parameters; none when the pack
    /// declares it more than once.
    fn pack_variable(&self, name: &str, doc_id: &str) -> Option<(&SymbolId, bool)> {
        let decision = doc_id.strip_prefix(DECISION)?.strip_prefix(':')?;
        let pack_doc = format!("{PACK}:{}", decision.rsplit_once('/')?.0);
        let mut candidates = self
            .packs
            .get(name)?
            .iter()
            .filter(|(owner, _, _)| *owner == pack_doc)
            .map(|(_, id, has_params)| (id, *has_params));
        let found = candidates.next()?;
        candidates.next().is_none().then_some(found)
    }

    fn walk(
        &mut self,
        at: &Expression,
        expr: &IdedExpr,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        match &expr.expr {
            Expr::Ident(name) => self.walk_ident(at, expr, name, bound, edges),
            Expr::Call(call) => self.walk_call(at, expr, call, bound, edges),
            Expr::Comprehension(comp) => self.walk_comprehension(at, comp, bound, edges),
            Expr::Select(_) => self.walk_select(at, expr, bound, edges),
            Expr::List(list) => {
                for item in &list.elements {
                    self.walk(at, item, bound, edges);
                }
            }
            Expr::Map(map) => self.walk_entries(at, &map.entries, bound, edges),
            Expr::Struct(object) => self.walk_entries(at, &object.entries, bound, edges),
            Expr::Literal(_) | Expr::Unspecified => {}
        }
    }

    fn walk_ident(
        &mut self,
        at: &Expression,
        expr: &IdedExpr,
        name: &str,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        if name.starts_with('@') || bound.contains(name) {
            return;
        }
        let parameter = at
            .params
            .iter()
            .find(|parameter| parameter.name == name)
            .map(|parameter| parameter.id.clone());
        let target = parameter
            .or_else(|| self.declared(name, at.doc_id))
            .or_else(|| {
                self.pack_variable(name, at.doc_id)
                    .map(|(id, _)| id.clone())
            });
        let (id, resolution) = match target {
            Some(id) => (id, Resolution::Syntactic),
            None if CONTEXT_VARIABLES.contains(&name) => return,
            None => (unresolved(name, SymbolKind::Var), Resolution::Heuristic),
        };
        edges.push(edge(
            EdgeKind::References,
            at.from,
            &id,
            expr_span(at.program, expr, at.mapped),
            resolution,
        ));
    }

    fn walk_call(
        &mut self,
        at: &Expression,
        expr: &IdedExpr,
        call: &CallExpr,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        let resolved = self.resolve_call(at, call);
        let namespaced = resolved.as_ref().is_some_and(|found| found.namespaced);
        if let Some(found) = resolved {
            edges.push(edge(
                EdgeKind::Calls,
                at.from,
                &found.id,
                call_span(at.program, expr, call, at.mapped),
                found.resolution,
            ));
        }
        if let Some(target) = &call.target
            && !namespaced
        {
            self.walk(at, target, bound, edges);
        }
        for arg in &call.args {
            self.walk(at, arg, bound, edges);
        }
    }

    /// What a call calls: a library function, a function of the document or of
    /// its pack, a standard function, or, by its name alone, something
    /// unknown. None for an operator.
    fn resolve_call(&mut self, at: &Expression, call: &CallExpr) -> Option<Resolved> {
        if call.func_name.starts_with('_') {
            return None;
        }
        let found = |id: SymbolId, namespaced| {
            Some(Resolved {
                id,
                resolution: Resolution::Syntactic,
                namespaced,
            })
        };
        if let Some(library) = library_target(call, self.libraries) {
            let id = library.symbol.id.clone();
            self.external.entry(id.clone()).or_insert(library.symbol);
            return found(id, library.namespaced);
        }
        let local = self.variables.iter().find(|variable| {
            variable.doc_id == at.doc_id
                && variable.name == call.func_name
                && !variable.params.is_empty()
        });
        if let Some(variable) = local {
            return found(variable.id.clone(), false);
        }
        if let Some((id, true)) = self.pack_variable(&call.func_name, at.doc_id) {
            return found(id.clone(), false);
        }
        if let Some(standard) = standard_target(call) {
            let id = standard.symbol.id.clone();
            self.external.entry(id.clone()).or_insert(standard.symbol);
            return found(id, standard.namespaced);
        }
        Some(Resolved {
            id: unresolved(&call.func_name, SymbolKind::Function),
            resolution: Resolution::Heuristic,
            namespaced: false,
        })
    }

    fn walk_comprehension(
        &mut self,
        at: &Expression,
        comp: &ComprehensionExpr,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        self.walk(at, &comp.iter_range, bound, edges);
        self.walk(at, &comp.accu_init, bound, edges);
        let mut nested = bound.clone();
        nested.insert(comp.iter_var.clone());
        if let Some(var) = &comp.iter_var2 {
            nested.insert(var.clone());
        }
        nested.insert(comp.accu_var.clone());
        self.walk(at, &comp.loop_cond, &nested, edges);
        self.walk(at, &comp.loop_step, &nested, edges);
        self.walk(at, &comp.result, &nested, edges);
    }

    fn walk_select(
        &mut self,
        at: &Expression,
        expr: &IdedExpr,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        let Expr::Select(select) = &expr.expr else {
            return;
        };
        self.walk(at, &select.operand, bound, edges);
        let id = SymbolId::new("cel/fields", &[], &select.field, SymbolKind::Field);
        self.external
            .entry(id.clone())
            .or_insert_with(|| external_symbol(id.clone(), SymbolKind::Field, &select.field, None));
        edges.push(edge(
            EdgeKind::References,
            at.from,
            &id,
            field_span(at.program, expr, &select.field, at.mapped),
            Resolution::Syntactic,
        ));
    }

    fn walk_entries(
        &mut self,
        at: &Expression,
        entries: &[IdedEntryExpr],
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        for entry in entries {
            match &entry.expr {
                EntryExpr::StructField(field) => self.walk(at, &field.value, bound, edges),
                EntryExpr::MapEntry(entry) => {
                    self.walk(at, &entry.key, bound, edges);
                    self.walk(at, &entry.value, bound, edges);
                }
            }
        }
    }
}

/// The symbol of a name that nothing declares.
fn unresolved(name: &str, kind: SymbolKind) -> SymbolId {
    SymbolId::new("cel/unresolved", &[], name, kind)
}

fn edge(
    kind: EdgeKind,
    from: &SymbolId,
    to: &SymbolId,
    site: Span,
    resolution: Resolution,
) -> Edge {
    Edge {
        kind,
        from: Node::Symbol(from.clone()),
        to: Target::Path(to.as_str().to_owned()),
        resolution,
        site: Some(site),
    }
}

#[cfg(test)]
mod tests;
