//! One function body: the calls and references it makes (resolved through the
//! name index and the receiver types that are syntactically known), its
//! normalized control flow, and the counts of its summary.

mod calls;
mod flow;

use std::collections::{HashMap, HashSet};

use lighthouse_protocol::{EdgeKind, Flow, FlowKind, Resolution, Span, SymbolKind};
use syn::{
    BinOp, Block, Expr, ExprCall, ExprClosure, ExprIf, ExprMatch, ExprMethodCall, ExprPath,
    ExprStruct, Item, Local, Member, Pat, Stmt, UnOp,
    visit::{self, Visit},
};

use flow::is_logic;

use crate::{
    macros,
    names::{Index, Ns, Res, Sym, TyCx, UseEntry, flatten, symbol_id},
    spans::ident_span,
    tree::SourceFile,
};

/// A method name shared by more private methods than this is too common for a
/// guess about an unknown receiver to mean anything.
const MAX_GUESSED_CANDIDATES: usize = 3;

/// The same limit for `pub` methods, which tests reach through values the
/// provider cannot type.
const MAX_GUESSED_PUBLIC_CANDIDATES: usize = 8;

/// A call or reference found in a body.
pub struct Use {
    pub to: String,
    pub resolution: Resolution,
}

/// A use at one place: the identifier that names the target.
pub struct Occurrence {
    pub kind: EdgeKind,
    pub to: String,
    pub resolution: Resolution,
    pub site: Span,
}

/// What the walk of one body found.
pub struct Facts {
    pub uses: Vec<Use>,
    /// Every place a use occurs, in walk order.
    pub sites: Vec<Occurrence>,
    /// Macro invocations in the body whose arguments are not expressions.
    pub unread_macros: u32,
    pub flow: Vec<Flow>,
    pub max_nesting: u32,
    pub statements: u32,
    /// Functions declared inside the body, to be reported as symbols of their
    /// own.
    pub nested: Vec<syn::ItemFn>,
    pub forwards_to: Option<String>,
}

/// Where a function lives: the module of its id, its owner path parts and the
/// type context its signature is read in.
pub struct Home {
    pub kind: SymbolKind,
    pub module_path: String,
    pub parts: Vec<String>,
    pub cx: TyCx,
}

enum Hit {
    Item(Sym),
    Assoc(Sym, String),
    Nested(String),
    Local,
    Unknown,
}

#[derive(Default)]
struct Scope {
    locals: HashMap<String, Option<Sym>>,
    items: HashMap<String, String>,
}

struct Walker<'i> {
    idx: &'i Index<'i>,
    file: &'i SourceFile,
    cx: TyCx,
    id: String,
    module_path: String,
    parts: Vec<String>,
    returns_value: bool,
    scopes: Vec<Scope>,
    extra: Vec<UseEntry>,
    nesting: u32,
    deepest: u32,
    count: u32,
    flow: Vec<Flow>,
    uses: Vec<Use>,
    sites: Vec<Occurrence>,
    unread_macros: u32,
    seen: HashSet<(EdgeKind, String, bool)>,
    nested: Vec<syn::ItemFn>,
    tails: HashSet<*const ExprMatch>,
}

impl<'ast> Visit<'ast> for Walker<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.scopes.push(Scope::default());
        self.register_items(block);
        let statements = block
            .stmts
            .iter()
            .filter(|s| !matches!(s, Stmt::Item(_)))
            .count();
        self.count += u32::try_from(statements).unwrap_or(u32::MAX);
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        match stmt {
            Stmt::Local(local) => self.local_stmt(local),
            Stmt::Item(_) => {}
            other => visit::visit_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        match e {
            Expr::If(i) => self.if_expr(i, false),
            Expr::Match(m) => self.match_expr(m),
            Expr::ForLoop(f) => self.for_loop(f),
            Expr::While(l) => self.while_loop(l),
            Expr::Loop(l) => self.plain_loop(l),
            Expr::Closure(c) => self.closure(c),
            Expr::Async(a) => self.nested_in(|w| w.visit_block(&a.block)),
            Expr::Binary(b) if is_logic(&b.op) => self.logic(e),
            Expr::Call(c) => self.call(c),
            Expr::MethodCall(m) => self.method_call(m),
            Expr::Path(p) => self.path_value(p),
            Expr::Struct(s) => self.struct_literal(s),
            Expr::Field(f) => self.field_access(f),
            Expr::Break(b) => self.jump(b.label.is_some(), |w| visit::visit_expr_break(w, b)),
            Expr::Continue(c) => self.jump(c.label.is_some(), |_| {}),
            Expr::Return(r) => self.return_expr(r),
            Expr::Let(l) => self.let_expr(l),
            other => visit::visit_expr(self, other),
        }
    }

    fn visit_type_path(&mut self, tp: &'ast syn::TypePath) {
        if tp.qself.is_none()
            && !tp.path.is_ident("Self")
            && let Hit::Item(sym) = self.hit(&tp.path, Ns::Type)
            && matches!(sym.kind, SymbolKind::Type | SymbolKind::Interface)
            && let Some(at) = last_ident(&tp.path)
        {
            self.add(EdgeKind::References, sym.id.clone(), at);
        }
        visit::visit_type_path(self, tp);
    }

    fn visit_pat_struct(&mut self, p: &'ast syn::PatStruct) {
        self.pattern_path(&p.path, Ns::Type);
        visit::visit_pat_struct(self, p);
    }

    fn visit_pat_tuple_struct(&mut self, p: &'ast syn::PatTupleStruct) {
        self.pattern_path(&p.path, Ns::Value);
        visit::visit_pat_tuple_struct(self, p);
    }

    /// Reached only through a path pattern: expression paths are taken by
    /// `path_value` before the walk gets here.
    fn visit_expr_path(&mut self, p: &'ast ExprPath) {
        self.pattern_path(&p.path, Ns::Value);
        visit::visit_expr_path(self, p);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        let Some(arguments) = macros::arguments(m) else {
            self.unread_macros += 1;
            return;
        };
        for argument in &arguments {
            self.visit_expr(argument);
        }
    }
}

impl<'i> Walker<'i> {
    fn mark_tail(&mut self, block: &Block) {
        if !self.returns_value {
            return;
        }
        if let Some(Stmt::Expr(Expr::Match(m), None)) = block.stmts.last() {
            self.tails.insert(std::ptr::from_ref(m));
        }
    }

    fn bind_params(&mut self, sig: &syn::Signature) {
        let scope = &mut self.scopes[0];
        for input in &sig.inputs {
            if let syn::FnArg::Receiver(_) = input {
                scope
                    .locals
                    .insert("self".to_owned(), self.cx.self_ty.clone());
            }
        }
        for input in &sig.inputs {
            if let syn::FnArg::Typed(typed) = input {
                let hint = self.idx.type_sym(&typed.ty, &self.cx);
                self.bind(&typed.pat, hint);
            }
        }
    }

    fn bind(&mut self, pat: &Pat, hint: Option<Sym>) {
        match pat {
            Pat::Ident(i) => {
                let name = i.ident.to_string();
                let constant = i.subpat.is_none()
                    && i.mutability.is_none()
                    && i.by_ref.is_none()
                    && name.starts_with(char::is_uppercase);
                if !constant {
                    self.declare_local(name, hint);
                }
                if let Some((_, sub)) = &i.subpat {
                    self.bind(sub, None);
                }
            }
            Pat::Type(t) => {
                let typed = self.idx.type_sym(&t.ty, &self.cx).or(hint);
                self.bind(&t.pat, typed);
            }
            Pat::Reference(r) => self.bind(&r.pat, hint),
            Pat::Paren(p) => self.bind(&p.pat, hint),
            Pat::Tuple(t) => t.elems.iter().for_each(|p| self.bind(p, None)),
            Pat::TupleStruct(t) => t.elems.iter().for_each(|p| self.bind(p, None)),
            Pat::Slice(t) => t.elems.iter().for_each(|p| self.bind(p, None)),
            Pat::Or(t) => t.cases.iter().for_each(|p| self.bind(p, None)),
            Pat::Struct(s) => s.fields.iter().for_each(|f| self.bind(&f.pat, None)),
            _ => {}
        }
    }

    fn declare_local(&mut self, name: String, hint: Option<Sym>) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.locals.insert(name, hint);
        }
    }

    fn pattern_path(&mut self, path: &syn::Path, ns: Ns) {
        let hit = self.hit(path, ns);
        if let Hit::Item(sym) = &hit
            && sym.kind != SymbolKind::Function
            && let Some(at) = last_ident(path)
        {
            self.add(EdgeKind::References, sym.id.clone(), at);
        }
    }

    fn hit(&self, path: &syn::Path, ns: Ns) -> Hit {
        let segs: Vec<String> = path
            .segments
            .iter()
            .map(|s| s.ident.to_string().trim_start_matches("r#").to_owned())
            .collect();
        let Some(first) = segs.first() else {
            return Hit::Unknown;
        };
        if segs.len() == 1 {
            if self.local(first).is_some() {
                return Hit::Local;
            }
            if let Some(id) = self.nested_item(first) {
                return Hit::Nested(id);
            }
        }
        if first == "Self" {
            return self.self_hit(&segs);
        }
        if let Some(bound) = self.cx.generics.get(first) {
            return match (segs.len(), bound) {
                (2, Some(_)) => self.bound_hit(first, &segs[1]),
                _ => Hit::Unknown,
            };
        }
        match self
            .idx
            .resolve_path(self.cx.module, &segs, ns, &self.extra)
        {
            Some(Res::Item(sym)) => Hit::Item(sym),
            Some(Res::Assoc(sym, name)) => Hit::Assoc(sym, name),
            _ => Hit::Unknown,
        }
    }

    fn local(&self, name: &str) -> Option<&Option<Sym>> {
        self.scopes.iter().rev().find_map(|s| s.locals.get(name))
    }

    fn nested_item(&self, name: &str) -> Option<String> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.items.get(name).cloned())
    }

    fn self_hit(&self, segs: &[String]) -> Hit {
        let Some(owner) = self.cx.self_ty.clone() else {
            return Hit::Unknown;
        };
        match segs {
            [_] => Hit::Item(owner),
            [_, member] => match self.idx.member(owner, member) {
                Res::Item(sym) => Hit::Item(sym),
                Res::Assoc(sym, name) => Hit::Assoc(sym, name),
                _ => Hit::Unknown,
            },
            _ => Hit::Unknown,
        }
    }

    /// `T::name` where `T` is a generic parameter bounded by a project trait.
    fn bound_hit(&self, param: &str, name: &str) -> Hit {
        let ty = syn::parse_str::<syn::Type>(param).ok();
        match ty.and_then(|t| self.idx.type_sym(&t, &self.cx)) {
            Some(sym) => Hit::Assoc(sym, name.to_owned()),
            None => Hit::Unknown,
        }
    }

    fn add(&mut self, kind: EdgeKind, to: String, at: &syn::Ident) {
        self.record(kind, to, Resolution::Syntactic, at);
    }

    /// Records a use: once among the distinct uses, and every time with the
    /// identifier that names the target.
    fn record(&mut self, kind: EdgeKind, to: String, resolution: Resolution, at: &syn::Ident) {
        let guess = resolution == Resolution::Heuristic;
        self.sites.push(Occurrence {
            kind,
            to: to.clone(),
            resolution,
            site: ident_span(self.file, at),
        });
        if self.seen.insert((kind, to.clone(), guess)) {
            self.uses.push(Use { to, resolution });
        }
    }

    fn local_stmt(&mut self, local: &Local) {
        let mut hint = None;
        if let Some(init) = &local.init {
            hint = self.expr_hint(&init.expr);
            self.visit_expr(&init.expr);
            if let Some((_, otherwise)) = &init.diverge {
                self.event(FlowKind::If);
                self.nested_in(|w| w.visit_expr(otherwise));
            }
        }
        self.visit_pat(&local.pat);
        self.bind(&local.pat, hint);
    }

    fn event(&mut self, kind: FlowKind) {
        self.flow.push(Flow {
            kind,
            nesting: self.nesting,
            arms: 0,
            operators: 0,
            returning: false,
        });
    }

    fn nested_in(&mut self, body: impl FnOnce(&mut Self)) {
        self.nesting += 1;
        self.deepest = self.deepest.max(self.nesting);
        body(self);
        self.nesting -= 1;
    }

    fn register_items(&mut self, block: &Block) {
        for stmt in &block.stmts {
            let Stmt::Item(item) = stmt else {
                continue;
            };
            match item {
                Item::Fn(f) => {
                    let name = f.sig.ident.to_string();
                    let mut parts: Vec<&str> = self.parts.iter().map(String::as_str).collect();
                    parts.push(&name);
                    let id = symbol_id(&self.module_path, &parts, SymbolKind::Function);
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.items.insert(name, id);
                    }
                    self.nested.push(f.clone());
                }
                Item::Use(u) => {
                    let mut entries = Vec::new();
                    flatten(&u.tree, &mut Vec::new(), &mut entries);
                    for (path, name) in entries {
                        self.extra.push(UseEntry {
                            vis: lighthouse_protocol::Visibility::Private,
                            path,
                            name,
                            module: self.cx.module,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn new(idx: &'i Index<'i>, file: &'i SourceFile, home: &Home, sig: &syn::Signature) -> Self {
        let parts: Vec<&str> = home.parts.iter().map(String::as_str).collect();
        Self {
            idx,
            file,
            cx: home.cx.clone(),
            id: symbol_id(&home.module_path, &parts, home.kind),
            module_path: home.module_path.clone(),
            parts: home.parts.clone(),
            returns_value: !matches!(sig.output, syn::ReturnType::Default),
            scopes: vec![Scope::default()],
            extra: Vec::new(),
            nesting: 0,
            deepest: 0,
            count: 0,
            flow: Vec::new(),
            uses: Vec::new(),
            sites: Vec::new(),
            unread_macros: 0,
            seen: HashSet::new(),
            nested: Vec::new(),
            tails: HashSet::new(),
        }
    }
}

/// Walks `block` and summarizes the calls, flow and counts of the body.
pub fn analyze(
    idx: &Index,
    file: &SourceFile,
    home: &Home,
    sig: &syn::Signature,
    block: &Block,
) -> Facts {
    let mut walker = Walker::new(idx, file, home, sig);
    walker.mark_tail(block);
    walker.bind_params(sig);
    walker.visit_block(block);
    let forwards_to = walker.forwards(sig, block);
    Facts {
        uses: walker.uses,
        sites: walker.sites,
        unread_macros: walker.unread_macros,
        flow: walker.flow,
        max_nesting: walker.deepest,
        statements: walker.count,
        nested: walker.nested,
        forwards_to,
    }
}

/// The identifier a path ends with, the one that names its target.
fn last_ident(path: &syn::Path) -> Option<&syn::Ident> {
    path.segments.last().map(|s| &s.ident)
}
