use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use cel::common::ast::{ComprehensionExpr, Expr, IdedExpr, LiteralValue};
use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, Flow, FlowKind, FunctionSummary, Node, Resolution, Signature, Symbol,
        SymbolId, SymbolKind, Target, TypeRef, Visibility,
    },
};
use lighthouse_plugin::CelLibraryManifest;

use super::host::{MappedText, Parameter, VariableDecl};

// Root values supplied by check, fix and recipe contexts, not by the host file.
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

const NAMESPACES: &[&str] = &["base64", "lists", "math", "optional", "strings"];
const GLOBALS: &[&str] = &[
    "bool",
    "bytes",
    "double",
    "duration",
    "int",
    "size",
    "string",
    "timestamp",
    "type",
    "uint",
    "has",
];
const MEMBERS: &[&str] = &[
    "charAt",
    "contains",
    "distinct",
    "endsWith",
    "flatten",
    "format",
    "getDate",
    "getDayOfMonth",
    "getDayOfWeek",
    "getDayOfYear",
    "getFullYear",
    "getHours",
    "getMilliseconds",
    "getMinutes",
    "getMonth",
    "getSeconds",
    "indexOf",
    "join",
    "lastIndexOf",
    "lowerAscii",
    "matches",
    "or",
    "orValue",
    "replace",
    "reverse",
    "slice",
    "sort",
    "sortBy",
    "size",
    "split",
    "startsWith",
    "substring",
    "trim",
    "upperAscii",
    "value",
    "hasValue",
];
const OPTIONALS: &[&str] = &["of", "none", "ofNonZeroValue"];
const NAMESPACE_FUNCTIONS: &[(&str, &[&str])] = &[
    ("base64", &["encode", "decode"]),
    ("lists", &["range"]),
    ("math", MATH),
    ("optional", OPTIONALS),
    ("strings", &["quote"]),
];

const MATH: &[&str] = &[
    "abs",
    "bitAnd",
    "bitNot",
    "bitOr",
    "bitShiftLeft",
    "bitShiftRight",
    "bitXor",
    "ceil",
    "floor",
    "isFinite",
    "isInf",
    "isNaN",
    "round",
    "sign",
    "sqrt",
    "trunc",
    "greatest",
    "least",
];

pub(super) type PackVariables = BTreeMap<String, Vec<(String, SymbolId, bool)>>;

pub(super) struct Analysis<'a> {
    variables: &'a [VariableDecl],
    packs: &'a PackVariables,
    libraries: &'a [CelLibraryManifest],
    external: &'a mut BTreeMap<SymbolId, Symbol>,
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
        let summary = Metrics::of(program.expression()).summary(
            from.clone(),
            params.len() as u32,
            token_count(&mapped.text),
            forwards_to(program, from, doc_id, self.variables, self.packs),
        );
        let mut edges = Vec::new();
        EdgeBuilder {
            program,
            mapped,
            from,
            doc_id,
            variables: self.variables,
            packs: self.packs,
            libraries: self.libraries,
            params,
            external: self.external,
        }
        .walk(program.expression(), &HashSet::new(), &mut edges);
        (summary, edges)
    }
}

#[derive(Default)]
struct Metrics {
    nodes: u32,
    max_nesting: u32,
    flow: Vec<Flow>,
}

impl Metrics {
    fn of(expr: &IdedExpr) -> Self {
        let mut out = Self::default();
        out.walk(expr, 0, None);
        out
    }

    fn walk(&mut self, expr: &IdedExpr, nesting: u32, parent_logic: Option<&str>) {
        self.nodes += 1;
        self.max_nesting = self.max_nesting.max(nesting);
        match &expr.expr {
            Expr::Call(call) => self.walk_call(expr, call, nesting, parent_logic),
            Expr::Comprehension(comp) => self.walk_comprehension(comp, nesting),
            Expr::Ident(_) | Expr::Literal(_) | Expr::Unspecified => {}
            Expr::Select(select) => self.walk(&select.operand, nesting, None),
            Expr::List(list) => self.walk_items(&list.elements, nesting),
            Expr::Map(map) => self.walk_entries(&map.entries, nesting),
            Expr::Struct(object) => self.walk_entries(&object.entries, nesting),
        }
    }

    fn walk_call(
        &mut self,
        expr: &IdedExpr,
        call: &cel::common::ast::CallExpr,
        nesting: u32,
        parent_logic: Option<&str>,
    ) {
        let logic = matches!(call.func_name.as_str(), "_&&_" | "_||_");
        let conditional = call.func_name == "_?_:_";
        if conditional {
            self.flow.push(Flow::new(FlowKind::If, nesting));
        } else if logic && parent_logic != Some(call.func_name.as_str()) {
            let mut flow = Flow::new(FlowKind::Logic, nesting);
            flow.operators = logic_run(expr, &call.func_name);
            self.flow.push(flow);
        }
        let same_logic = logic && parent_logic == Some(call.func_name.as_str());
        let next_nesting = nesting + u32::from(conditional || (logic && !same_logic));
        let child_logic = logic.then_some(call.func_name.as_str());
        if let Some(target) = &call.target {
            self.walk(target, next_nesting, child_logic);
        }
        for arg in &call.args {
            self.walk(arg, next_nesting, child_logic);
        }
    }

    fn walk_comprehension(&mut self, comp: &ComprehensionExpr, nesting: u32) {
        self.flow.push(Flow::new(FlowKind::Loop, nesting));
        if !is_boolean_literal(&comp.loop_cond) {
            self.flow.push(Flow::new(FlowKind::If, nesting + 1));
        }
        self.walk_comp(comp, nesting + 1);
    }

    fn walk_items(&mut self, items: &[IdedExpr], nesting: u32) {
        for item in items {
            self.walk(item, nesting, None);
        }
    }

    fn walk_entries(&mut self, entries: &[cel::common::ast::IdedEntryExpr], nesting: u32) {
        for entry in entries {
            match &entry.expr {
                cel::common::ast::EntryExpr::StructField(field) => {
                    self.walk(&field.value, nesting, None);
                }
                cel::common::ast::EntryExpr::MapEntry(entry) => {
                    self.walk(&entry.key, nesting, None);
                    self.walk(&entry.value, nesting, None);
                }
            }
        }
    }

    fn walk_comp(&mut self, comp: &ComprehensionExpr, nesting: u32) {
        self.walk(&comp.iter_range, nesting, None);
        self.walk(&comp.accu_init, nesting, None);
        self.walk(&comp.loop_cond, nesting, None);
        self.walk(&comp.loop_step, nesting, None);
        self.walk(&comp.result, nesting, None);
    }

    fn summary(
        self,
        symbol: SymbolId,
        params: u32,
        tokens: u32,
        forwards_to: Option<Target>,
    ) -> FunctionSummary {
        FunctionSummary {
            symbol,
            max_nesting: self.max_nesting,
            statements: self.nodes,
            top_level: 1,
            params,
            returns: 1,
            tokens,
            flow: self.flow,
            forwards_to,
            signature: Signature {
                params: vec![
                    TypeRef {
                        text: "dyn".to_owned(),
                        symbol: None,
                        exported: None,
                    };
                    params as usize
                ],
                results: vec![TypeRef {
                    text: "dyn".to_owned(),
                    symbol: None,
                    exported: None,
                }],
            },
            events: Vec::new(),
            manual_assertions: 0,
            implementation: false,
            constructs: false,
        }
    }
}

struct EdgeBuilder<'a> {
    program: &'a cel::Program,
    mapped: &'a MappedText,
    from: &'a SymbolId,
    doc_id: &'a str,
    variables: &'a [VariableDecl],
    packs: &'a PackVariables,
    libraries: &'a [CelLibraryManifest],
    params: &'a [Parameter],
    external: &'a mut BTreeMap<SymbolId, Symbol>,
}

impl EdgeBuilder<'_> {
    fn walk(&mut self, expr: &IdedExpr, bound: &HashSet<String>, edges: &mut Vec<Edge>) {
        match &expr.expr {
            Expr::Ident(name) => self.walk_ident(expr, name, bound, edges),
            Expr::Call(call) => self.walk_call(expr, call, bound, edges),
            Expr::Comprehension(comp) => self.walk_comprehension(comp, bound, edges),
            Expr::Select(_) => self.walk_select(expr, bound, edges),
            Expr::List(list) => self.walk_items(&list.elements, bound, edges),
            Expr::Map(map) => self.walk_entries(&map.entries, bound, edges),
            Expr::Struct(object) => self.walk_entries(&object.entries, bound, edges),
            Expr::Literal(_) | Expr::Unspecified => {}
        }
    }

    fn walk_ident(
        &mut self,
        expr: &IdedExpr,
        name: &str,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        if name.starts_with('@') || bound.contains(name) {
            return;
        }
        let local = self
            .variables
            .iter()
            .find(|variable| variable.doc_id == self.doc_id && variable.name == name)
            .map(|variable| variable.id.clone());
        let parameter = self
            .params
            .iter()
            .find(|parameter| parameter.name == name)
            .map(|parameter| parameter.id.clone());
        let target = parameter
            .or(local)
            .or_else(|| pack_target(name, self.packs, self.doc_id));
        let Some(target) = target else {
            if !CONTEXT_VARIABLES.contains(&name) {
                let id = SymbolId::new("cel/unresolved", &[], name, SymbolKind::Var);
                edges.push(edge(
                    EdgeKind::References,
                    self.from,
                    &id,
                    expr_span(self.program, expr, self.mapped),
                    Resolution::Heuristic,
                ));
            }
            return;
        };
        edges.push(edge(
            EdgeKind::References,
            self.from,
            &target,
            expr_span(self.program, expr, self.mapped),
            Resolution::Syntactic,
        ));
    }

    fn walk_call(
        &mut self,
        expr: &IdedExpr,
        call: &cel::common::ast::CallExpr,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        let (target, namespaced) = self.resolve_call(call);
        if let Some(target) = target {
            let resolution = if target.as_str().starts_with("cel/unresolved::") {
                Resolution::Heuristic
            } else {
                Resolution::Syntactic
            };
            edges.push(edge(
                EdgeKind::Calls,
                self.from,
                &target,
                call_span(self.program, expr, call, self.mapped),
                resolution,
            ));
        }
        if let Some(target) = &call.target
            && !namespaced
        {
            self.walk(target, bound, edges);
        }
        for arg in &call.args {
            self.walk(arg, bound, edges);
        }
    }

    fn resolve_call(&mut self, call: &cel::common::ast::CallExpr) -> (Option<SymbolId>, bool) {
        if call.func_name.starts_with('_') {
            return (None, false);
        }
        let (library_id, library, library_namespace) = library_target(call, self.libraries);
        let local = self
            .variables
            .iter()
            .find(|variable| {
                variable.doc_id == self.doc_id
                    && variable.name == call.func_name
                    && !variable.params.is_empty()
            })
            .map(|variable| variable.id.clone());
        let pack = pack_target(&call.func_name, self.packs, self.doc_id).filter(|id| {
            self.packs
                .get(&call.func_name)
                .into_iter()
                .flatten()
                .any(|(_, candidate, has_params)| candidate == id && *has_params)
        });
        let mut standard = None;
        let target = library_id
            .or(local)
            .or(pack)
            .or_else(|| {
                standard = standard_target(call);
                standard.as_ref().map(|(id, _, _)| id.clone())
            })
            .unwrap_or_else(|| {
                SymbolId::new("cel/unresolved", &[], &call.func_name, SymbolKind::Function)
            });
        if let Some(function) = library {
            self.external.entry(target.clone()).or_insert_with(|| {
                external_symbol(
                    target.clone(),
                    function_name(function),
                    SymbolKind::Function,
                    Some(function.doc.as_str()),
                )
            });
        }
        let namespaced = library_namespace
            || standard
                .as_ref()
                .is_some_and(|(_, _, namespace)| *namespace);
        if let Some((_, symbol, _)) = standard {
            self.external.entry(target.clone()).or_insert(symbol);
        }
        (Some(target), namespaced)
    }

    fn walk_comprehension(
        &mut self,
        comp: &ComprehensionExpr,
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        self.walk(&comp.iter_range, bound, edges);
        self.walk(&comp.accu_init, bound, edges);
        let mut nested = bound.clone();
        nested.insert(comp.iter_var.clone());
        if let Some(var) = &comp.iter_var2 {
            nested.insert(var.clone());
        }
        nested.insert(comp.accu_var.clone());
        self.walk(&comp.loop_cond, &nested, edges);
        self.walk(&comp.loop_step, &nested, edges);
        self.walk(&comp.result, &nested, edges);
    }

    fn walk_select(&mut self, expr: &IdedExpr, bound: &HashSet<String>, edges: &mut Vec<Edge>) {
        let Expr::Select(select) = &expr.expr else {
            return;
        };
        self.walk(&select.operand, bound, edges);
        let id = SymbolId::new("cel/fields", &[], &select.field, SymbolKind::Field);
        self.external
            .entry(id.clone())
            .or_insert_with(|| external_symbol(id.clone(), &select.field, SymbolKind::Field, None));
        edges.push(edge(
            EdgeKind::References,
            self.from,
            &id,
            field_span(self.program, expr, &select.field, self.mapped),
            Resolution::Syntactic,
        ));
    }

    fn walk_items(&mut self, items: &[IdedExpr], bound: &HashSet<String>, edges: &mut Vec<Edge>) {
        for item in items {
            self.walk(item, bound, edges);
        }
    }

    fn walk_entries(
        &mut self,
        entries: &[cel::common::ast::IdedEntryExpr],
        bound: &HashSet<String>,
        edges: &mut Vec<Edge>,
    ) {
        for entry in entries {
            match &entry.expr {
                cel::common::ast::EntryExpr::StructField(field) => {
                    self.walk(&field.value, bound, edges);
                }
                cel::common::ast::EntryExpr::MapEntry(entry) => {
                    self.walk(&entry.key, bound, edges);
                    self.walk(&entry.value, bound, edges);
                }
            }
        }
    }
}

pub(super) fn function_catalog(libraries: &[CelLibraryManifest]) -> BTreeMap<SymbolId, Symbol> {
    let mut symbols = BTreeMap::new();
    for name in GLOBALS {
        let symbol = standard_symbol("cel/stdlib", None, name);
        symbols.insert(symbol.id.clone(), symbol);
    }
    for name in MEMBERS {
        let symbol = standard_symbol("cel/stdlib/member", Some("receiver"), name);
        symbols.insert(symbol.id.clone(), symbol);
    }
    for (namespace, names) in NAMESPACE_FUNCTIONS {
        for name in *names {
            let module = format!("cel/stdlib/{namespace}");
            let symbol = standard_symbol(&module, Some("global"), name);
            symbols.insert(symbol.id.clone(), symbol);
        }
    }
    for library in libraries {
        for function in &library.functions {
            let id = library_symbol_id(&library.id, function);
            symbols.entry(id.clone()).or_insert_with(|| {
                external_symbol(
                    id,
                    function_name(function),
                    SymbolKind::Function,
                    Some(function.doc.as_str()),
                )
            });
        }
    }
    symbols
}

pub(super) fn external_module(symbol: &Symbol) -> String {
    symbol.id.module().to_owned()
}

fn standard_symbol(module: &str, owner: Option<&str>, name: &str) -> Symbol {
    let owner = owner.into_iter().collect::<Vec<_>>();
    let id = SymbolId::new(module, &owner, name, SymbolKind::Function);
    external_symbol(id, name, SymbolKind::Function, None)
}

fn logic_run(expr: &IdedExpr, operator: &str) -> u32 {
    match &expr.expr {
        Expr::Call(call) if call.func_name == operator => {
            1 + call
                .target
                .iter()
                .map(Box::as_ref)
                .chain(call.args.iter())
                .map(|child| logic_run(child, operator))
                .sum::<u32>()
        }
        _ => 0,
    }
}

fn is_boolean_literal(expr: &IdedExpr) -> bool {
    matches!(&expr.expr, Expr::Literal(LiteralValue::Boolean(_)))
}

fn token_count(source: &str) -> u32 {
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut count = 0u32;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        count += 1;
        if byte == b'\'' || byte == b'"' {
            let quote = byte;
            at += 1;
            while at < bytes.len() {
                if bytes[at] == b'\\' {
                    at = (at + 2).min(bytes.len());
                } else if bytes[at] == quote {
                    at += 1;
                    break;
                } else {
                    at += 1;
                }
            }
        } else if byte.is_ascii_alphanumeric() || byte == b'_' {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
        } else if at + 1 < bytes.len()
            && matches!(
                &bytes[at..at + 2],
                b"&&" | b"||" | b"==" | b"!=" | b"<=" | b">=" | b"??" | b"?."
            )
        {
            at += 2;
        } else {
            at += 1;
        }
    }
    count
}

fn forwards_to(
    program: &cel::Program,
    from: &SymbolId,
    doc_id: &str,
    variables: &[VariableDecl],
    packs: &PackVariables,
) -> Option<Target> {
    let variable = variables
        .iter()
        .find(|variable| variable.id == *from && variable.doc_id == doc_id)?;
    if !variable.params.is_empty() {
        return None;
    }
    let Expr::Ident(name) = &program.expression().expr else {
        return None;
    };
    let target = variables
        .iter()
        .find(|candidate| candidate.doc_id == doc_id && candidate.name == *name)
        .map(|candidate| candidate.id.clone())
        .or_else(|| pack_target(name, packs, doc_id))?;
    (target != *from).then(|| Target::Path(target.as_str().to_owned()))
}

fn pack_target(name: &str, packs: &PackVariables, doc_id: &str) -> Option<SymbolId> {
    let pack = doc_id.strip_prefix("Decision:")?.rsplit_once('/')?.0;
    let pack_doc = format!("Pack:{pack}");
    let mut candidates = packs
        .get(name)?
        .iter()
        .filter(|(owner, _, _)| owner == &pack_doc)
        .map(|(_, id, _)| id.clone());
    let target = candidates.next()?;
    candidates.next().is_none().then_some(target)
}

fn standard_target(call: &cel::common::ast::CallExpr) -> Option<(SymbolId, Symbol, bool)> {
    let target_namespace = call.target.as_ref().and_then(|target| match &target.expr {
        Expr::Ident(name) if NAMESPACES.contains(&name.as_str()) => Some(name.as_str()),
        _ => None,
    });
    let (namespace, name, is_namespace) = if let Some(namespace) = target_namespace {
        let name = call.func_name.rsplit('.').next().unwrap_or(&call.func_name);
        (Some(namespace), name, true)
    } else if let Some((namespace, name)) = call.func_name.rsplit_once('.') {
        if NAMESPACES.contains(&namespace) {
            (Some(namespace), name, false)
        } else {
            (None, call.func_name.as_str(), false)
        }
    } else {
        (None, call.func_name.as_str(), false)
    };

    let (module, owner, symbol_name) = if let Some(namespace) = namespace {
        let known = NAMESPACE_FUNCTIONS
            .iter()
            .any(|(candidate, names)| *candidate == namespace && names.contains(&name));
        if !known {
            return None;
        }
        (format!("cel/stdlib/{namespace}"), Some("global"), name)
    } else if call.target.is_none() && GLOBALS.contains(&name) {
        ("cel/stdlib".to_owned(), None, name)
    } else if call.target.is_some() && MEMBERS.contains(&name) {
        ("cel/stdlib/member".to_owned(), Some("receiver"), name)
    } else {
        return None;
    };

    let symbol = standard_symbol(&module, owner, symbol_name);
    let id = symbol.id.clone();
    Some((id, symbol, is_namespace))
}

fn library_target<'a>(
    call: &cel::common::ast::CallExpr,
    libraries: &'a [CelLibraryManifest],
) -> (
    Option<SymbolId>,
    Option<&'a lighthouse_plugin::Function>,
    bool,
) {
    let namespace = call.target.as_ref().and_then(|target| match &target.expr {
        Expr::Ident(name) => Some(name.as_str()),
        _ => None,
    });
    let name = call.func_name.rsplit('.').next().unwrap_or(&call.func_name);
    let qualified = namespace.map(|namespace| format!("{namespace}.{name}"));
    for library in libraries {
        for function in &library.functions {
            if function.receiver.is_none()
                && (function.name == call.func_name
                    || qualified.as_deref() == Some(function.name.as_str()))
            {
                let namespaced = namespace
                    .is_some_and(|namespace| function.name == format!("{namespace}.{name}"));
                return (
                    Some(library_symbol_id(&library.id, function)),
                    Some(function),
                    namespaced,
                );
            }
            if function.name == call.func_name
                && function.receiver.is_some()
                && call.target.is_some()
            {
                return (
                    Some(library_symbol_id(&library.id, function)),
                    Some(function),
                    false,
                );
            }
        }
    }
    (None, None, false)
}

fn library_symbol_id(library: &str, function: &lighthouse_plugin::Function) -> SymbolId {
    let module = library_module(library);
    let receiver = function.receiver.as_deref().unwrap_or("global");
    SymbolId::new(
        &module,
        &[receiver],
        function_name(function),
        SymbolKind::Function,
    )
}

fn library_module(library: &str) -> String {
    format!("cel/library/{library}")
}

fn function_name(function: &lighthouse_plugin::Function) -> &str {
    function.name.rsplit('.').next().unwrap_or(&function.name)
}

fn external_symbol(id: SymbolId, name: &str, kind: SymbolKind, doc: Option<&str>) -> Symbol {
    Symbol {
        id,
        kind,
        visibility: Visibility::Public,
        owner: None,
        file: PathBuf::from("@cel/library"),
        span: Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 1 },
        },
        extent: None,
        doc: doc.map(str::to_owned),
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
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

fn expr_span(program: &cel::Program, expr: &IdedExpr, mapped: &MappedText) -> Span {
    program
        .source_info()
        .offset_for(expr.id)
        .map(|(start, stop)| mapped.span(start as usize, stop as usize + 1, &mapped.host))
        .unwrap_or_else(|| mapped.span(0, mapped.text.len(), &mapped.host))
}

fn call_span(
    program: &cel::Program,
    expr: &IdedExpr,
    call: &cel::common::ast::CallExpr,
    mapped: &MappedText,
) -> Span {
    let name = call.func_name.rsplit('.').next().unwrap_or(&call.func_name);
    if let Some((start, _)) = program.source_info().offset_for(expr.id) {
        let end = (start as usize).min(mapped.text.len());
        if end >= name.len() && mapped.text.get(end - name.len()..end) == Some(name) {
            return mapped.span(end - name.len(), end, &mapped.host);
        }
    }
    let whole = program
        .source_info()
        .offset_for(expr.id)
        .map(|(start, stop)| (start as usize, stop as usize + 1))
        .unwrap_or((0, mapped.text.len()));
    let start = whole.0.min(mapped.text.len());
    let end = whole.1.min(mapped.text.len()).max(start);
    mapped.text[start..end]
        .rfind(name)
        .map(|at| mapped.span(start + at, start + at + name.len(), &mapped.host))
        .unwrap_or_else(|| mapped.span(start, end, &mapped.host))
}

fn field_span(program: &cel::Program, expr: &IdedExpr, field: &str, mapped: &MappedText) -> Span {
    let whole = program
        .source_info()
        .offset_for(expr.id)
        .map(|(start, stop)| (start as usize, stop as usize + 1))
        .unwrap_or((0, mapped.text.len()));
    let start = whole.0.min(mapped.text.len());
    let end = whole.1.min(mapped.text.len()).max(start);
    let token = mapped.text.get(start..).unwrap_or("");
    let field_start = if token.starts_with(".?") || token.starts_with("?.") {
        Some(start + 2)
    } else if token.starts_with('.') {
        Some(start + 1)
    } else {
        None
    };
    field_start
        .filter(|at| {
            mapped
                .text
                .get(*at..)
                .is_some_and(|rest| rest.starts_with(field))
        })
        .map(|at| mapped.span(at, at + field.len(), &mapped.host))
        .or_else(|| {
            mapped.text[start..end]
                .rfind(field)
                .map(|at| mapped.span(start + at, start + at + field.len(), &mapped.host))
        })
        .unwrap_or_else(|| mapped.span(start, end, &mapped.host))
}
