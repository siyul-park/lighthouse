//! One function body: the calls and references it makes (resolved through the
//! name index and the receiver types that are syntactically known), its
//! normalized control flow, and the counts of its summary.

use std::collections::{HashMap, HashSet};

use lighthouse_protocol::{EdgeKind, Flow, FlowKind, Resolution, SymbolKind};
use syn::{
    BinOp, Block, Expr, ExprCall, ExprClosure, ExprIf, ExprMatch, ExprMethodCall, ExprPath,
    ExprStruct, Item, Local, Member, Pat, Stmt, UnOp,
    visit::{self, Visit},
};

use crate::{
    macros,
    names::{Index, Ns, Res, Sym, TyCx, UseEntry, flatten, symbol_id},
};

/// A method name shared by more private methods than this is too common for a
/// guess about an unknown receiver to mean anything.
const MAX_GUESSED_CANDIDATES: usize = 3;

/// The same limit for `pub` methods, which tests reach through values the
/// provider cannot type.
const MAX_GUESSED_PUBLIC_CANDIDATES: usize = 8;

/// A call or reference found in a body.
pub struct Use {
    pub kind: EdgeKind,
    pub to: String,
    pub resolution: Resolution,
}

/// What the walk of one body found.
pub struct Facts {
    pub uses: Vec<Use>,
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
        {
            self.add(EdgeKind::References, sym.id.clone());
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
        {
            self.add(EdgeKind::References, sym.id.clone());
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

    fn add(&mut self, kind: EdgeKind, to: String) {
        self.record(kind, to, Resolution::Syntactic);
    }

    fn record(&mut self, kind: EdgeKind, to: String, resolution: Resolution) {
        let guess = resolution == Resolution::Heuristic;
        if self.seen.insert((kind, to.clone(), guess)) {
            self.uses.push(Use {
                kind,
                to,
                resolution,
            });
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

    fn expr_hint(&self, e: &Expr) -> Option<Sym> {
        match e {
            Expr::Path(p) if p.qself.is_none() => {
                let ident = p.path.get_ident()?;
                self.local(&ident.to_string()).cloned().flatten()
            }
            Expr::Reference(r) => self.expr_hint(&r.expr),
            Expr::Paren(p) => self.expr_hint(&p.expr),
            Expr::Group(g) => self.expr_hint(&g.expr),
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => self.expr_hint(&u.expr),
            Expr::Cast(c) => self.idx.type_sym(&c.ty, &self.cx),
            Expr::Field(f) => {
                let owner = self.expr_hint(&f.base)?;
                let Member::Named(name) = &f.member else {
                    return None;
                };
                let field = self.idx.field(&owner, &name.to_string())?;
                self.idx.type_sym(&field.ty, &field.cx)
            }
            Expr::MethodCall(m) => self.method_hint(m, false),
            Expr::Call(c) => self.call_hint(c, false),
            Expr::Struct(s) => self.struct_hint(s),
            Expr::Try(t) => self.try_hint(&t.expr),
            Expr::Await(a) => self.expr_hint(&a.base),
            _ => None,
        }
    }

    fn method_hint(&self, m: &ExprMethodCall, unwrap: bool) -> Option<Sym> {
        let recv = self.expr_hint(&m.receiver)?;
        let id = self.idx.method(&recv, &m.method.to_string())?;
        self.idx.return_sym(&id, unwrap)
    }

    fn call_hint(&self, c: &ExprCall, unwrap: bool) -> Option<Sym> {
        let Expr::Path(p) = &*c.func else {
            return None;
        };
        match self.hit(&p.path, Ns::Value) {
            Hit::Item(sym) => match sym.kind {
                SymbolKind::Function => self.idx.return_sym(&sym.id, unwrap),
                SymbolKind::Type if !unwrap => Some(sym),
                SymbolKind::Field if !unwrap => self.idx.variant_owner.get(&sym.id).cloned(),
                _ => None,
            },
            Hit::Assoc(sym, name) => {
                let id = self.idx.method(&sym, &name)?;
                self.idx.return_sym(&id, unwrap)
            }
            _ => None,
        }
    }

    fn struct_hint(&self, s: &ExprStruct) -> Option<Sym> {
        match self.hit(&s.path, Ns::Type) {
            Hit::Item(sym) if sym.kind == SymbolKind::Type => Some(sym),
            Hit::Item(sym) if sym.kind == SymbolKind::Field => {
                self.idx.variant_owner.get(&sym.id).cloned()
            }
            _ => None,
        }
    }

    fn try_hint(&self, inner: &Expr) -> Option<Sym> {
        match inner {
            Expr::Await(a) => self.try_hint(&a.base),
            Expr::Call(c) => self.call_hint(c, true),
            Expr::MethodCall(m) => self.method_hint(m, true),
            _ => None,
        }
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
                            vis: crate::tree::Vis::Private,
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

    fn new(idx: &'i Index<'i>, home: &Home, sig: &syn::Signature) -> Self {
        let parts: Vec<&str> = home.parts.iter().map(String::as_str).collect();
        Self {
            idx,
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
            unread_macros: 0,
            seen: HashSet::new(),
            nested: Vec::new(),
            tails: HashSet::new(),
        }
    }

    fn if_expr(&mut self, i: &ExprIf, chained: bool) {
        self.event(if chained {
            FlowKind::ElseIf
        } else {
            FlowKind::If
        });
        self.scoped(|w| {
            w.visit_expr(&i.cond);
            w.nested_in(|w| w.visit_block(&i.then_branch));
        });
        match i.else_branch.as_ref().map(|(_, e)| &**e) {
            Some(Expr::If(next)) => self.if_expr(next, true),
            Some(Expr::Block(b)) => {
                self.event(FlowKind::Else);
                self.nested_in(|w| w.visit_block(&b.block));
            }
            Some(other) => self.visit_expr(other),
            None => {}
        }
    }

    fn scoped(&mut self, body: impl FnOnce(&mut Self)) {
        self.scopes.push(Scope::default());
        body(self);
        self.scopes.pop();
    }

    fn match_expr(&mut self, m: &ExprMatch) {
        let arms = m.arms.iter().filter(|a| !is_default(a)).count();
        self.flow.push(Flow {
            kind: FlowKind::Switch,
            nesting: self.nesting,
            arms: u32::try_from(arms).unwrap_or(u32::MAX),
            operators: 0,
            returning: self.match_returning(m),
        });
        self.visit_expr(&m.expr);
        self.nested_in(|w| {
            for arm in &m.arms {
                w.count += 1;
                w.scoped(|w| {
                    w.bind(&arm.pat, None);
                    w.visit_pat(&arm.pat);
                    if let Some((_, guard)) = &arm.guard {
                        w.visit_expr(guard);
                    }
                    w.visit_expr(&arm.body);
                });
            }
        });
    }

    /// A table in code form: every arm yields a value without further
    /// statements, in return position, or every arm is a `return`.
    fn match_returning(&self, m: &ExprMatch) -> bool {
        if m.arms.is_empty() || !m.arms.iter().all(|a| single_value(&a.body)) {
            return false;
        }
        m.arms.iter().all(|a| is_return(&a.body)) || self.tails.contains(&std::ptr::from_ref(m))
    }

    fn closure(&mut self, c: &ExprClosure) {
        self.scoped(|w| {
            for input in &c.inputs {
                let hint = match input {
                    Pat::Type(t) => w.idx.type_sym(&t.ty, &w.cx),
                    _ => None,
                };
                w.bind(input, hint);
                w.visit_pat(input);
            }
            w.nested_in(|w| w.visit_expr(&c.body));
        });
    }

    fn logic(&mut self, root: &Expr) {
        let mut ops = Vec::new();
        let mut leaves = Vec::new();
        flatten_logic(root, &mut ops, &mut leaves);
        let mut at = 0;
        while at < ops.len() {
            let run = ops[at..]
                .iter()
                .take_while(|o| std::mem::discriminant(*o) == std::mem::discriminant(&ops[at]))
                .count();
            self.flow.push(Flow {
                kind: FlowKind::Logic,
                nesting: self.nesting,
                arms: 0,
                operators: u32::try_from(run).unwrap_or(u32::MAX),
                returning: false,
            });
            at += run;
        }
        for leaf in leaves {
            self.visit_expr(leaf);
        }
    }

    fn for_loop(&mut self, f: &syn::ExprForLoop) {
        self.event(FlowKind::Loop);
        self.visit_expr(&f.expr);
        self.scoped(|w| {
            w.bind(&f.pat, None);
            w.visit_pat(&f.pat);
            w.nested_in(|w| w.visit_block(&f.body));
        });
    }

    fn while_loop(&mut self, l: &syn::ExprWhile) {
        self.event(FlowKind::Loop);
        self.scoped(|w| {
            w.visit_expr(&l.cond);
            w.nested_in(|w| w.visit_block(&l.body));
        });
    }

    fn plain_loop(&mut self, l: &syn::ExprLoop) {
        self.event(FlowKind::Loop);
        self.nested_in(|w| w.visit_block(&l.body));
    }

    /// `break 'label` and `continue 'label` jump; plain ones do not.
    fn jump(&mut self, labeled: bool, rest: impl FnOnce(&mut Self)) {
        if labeled {
            self.event(FlowKind::Jump);
        }
        rest(self);
    }

    /// `return match ...` puts the match in return position.
    fn return_expr(&mut self, r: &syn::ExprReturn) {
        if let Some(Expr::Match(m)) = r.expr.as_deref() {
            self.tails.insert(std::ptr::from_ref(m));
        }
        visit::visit_expr_return(self, r);
    }

    fn let_expr(&mut self, l: &syn::ExprLet) {
        let hint = self.expr_hint(&l.expr);
        self.visit_expr(&l.expr);
        self.visit_pat(&l.pat);
        self.bind(&l.pat, hint);
    }

    fn call(&mut self, c: &ExprCall) {
        if let Expr::Path(p) = &*c.func {
            let hit = self.hit(&p.path, Ns::Value);
            if let Some((kind, id)) = self.call_target(&hit) {
                let recursive = kind == EdgeKind::Calls && id == self.id;
                self.add(kind, id);
                if recursive {
                    self.event(FlowKind::Recursion);
                }
            }
            self.visit_path(&p.path);
        } else {
            self.visit_expr(&c.func);
        }
        for arg in &c.args {
            self.visit_expr(arg);
        }
    }

    fn call_target(&self, hit: &Hit) -> Option<(EdgeKind, String)> {
        match hit {
            Hit::Item(sym) => match sym.kind {
                SymbolKind::Function => Some((EdgeKind::Calls, sym.id.clone())),
                SymbolKind::Type | SymbolKind::Field | SymbolKind::Const | SymbolKind::Var => {
                    Some((EdgeKind::References, sym.id.clone()))
                }
                _ => None,
            },
            Hit::Assoc(sym, name) => self.assoc_target(sym, name, EdgeKind::Calls),
            Hit::Nested(id) => Some((EdgeKind::Calls, id.clone())),
            Hit::Local | Hit::Unknown => None,
        }
    }

    fn assoc_target(
        &self,
        sym: &Sym,
        name: &str,
        method_kind: EdgeKind,
    ) -> Option<(EdgeKind, String)> {
        if let Some(id) = self.idx.method(sym, name) {
            return Some((method_kind, id));
        }
        let id = self.idx.assoc_const(sym, name)?;
        Some((EdgeKind::References, id))
    }

    fn method_call(&mut self, m: &ExprMethodCall) {
        let name = m.method.to_string();
        match self.expr_hint(&m.receiver) {
            Some(recv) => {
                if let Some(id) = self.idx.method(&recv, &name) {
                    let recursive = id == self.id;
                    self.add(EdgeKind::Calls, id);
                    if recursive {
                        self.event(FlowKind::Recursion);
                    }
                }
            }
            None => self.may_reference(&name),
        }
        self.visit_expr(&m.receiver);
        if let Some(turbofish) = &m.turbofish {
            self.visit_angle_bracketed_generic_arguments(turbofish);
        }
        for arg in &m.args {
            self.visit_expr(arg);
        }
    }

    /// A method call on a value of unknown type may reach any inherent method
    /// of that name in the crate. Such candidates are recorded as heuristic
    /// references, never as calls, so a rule counting callers does not mistake
    /// a method with an unseen caller for an unused one. A name shared by more
    /// than a few methods says nothing and is skipped; a `pub` method is
    /// guessed at a wider limit, since only tests that exercise it matter.
    fn may_reference(&mut self, name: &str) {
        let crates = &self.idx.tree.crates;
        let krate = self.idx.tree.mods[self.cx.module].krate;
        let mut private = Vec::new();
        let mut public = Vec::new();
        for (id, module, is_public) in self.idx.inherent_methods.get(name).into_iter().flatten() {
            let other = self.idx.tree.mods[*module].krate;
            // An integration test is a crate of its own, in the package of the
            // library it exercises.
            if *is_public && crates[other].package == crates[krate].package && *id != self.id {
                public.push(id.clone());
            } else if !is_public && other == krate {
                private.push(id.clone());
            }
        }
        let guesses = [
            (private, MAX_GUESSED_CANDIDATES),
            (public, MAX_GUESSED_PUBLIC_CANDIDATES),
        ];
        for (ids, limit) in guesses {
            if ids.len() > limit {
                continue;
            }
            for id in ids {
                self.record(EdgeKind::References, id, Resolution::Heuristic);
            }
        }
    }

    fn path_value(&mut self, p: &ExprPath) {
        if p.qself.is_none() {
            let hit = self.hit(&p.path, Ns::Value);
            if let Some((kind, id)) = self.value_target(&hit) {
                self.add(kind, id);
            }
        }
        visit::visit_expr_path(self, p);
    }

    fn value_target(&self, hit: &Hit) -> Option<(EdgeKind, String)> {
        match hit {
            Hit::Item(sym) if sym.kind != SymbolKind::Test => {
                Some((EdgeKind::References, sym.id.clone()))
            }
            Hit::Assoc(sym, name) => self.assoc_target(sym, name, EdgeKind::References),
            Hit::Nested(id) => Some((EdgeKind::References, id.clone())),
            _ => None,
        }
    }

    fn struct_literal(&mut self, s: &ExprStruct) {
        match self.hit(&s.path, Ns::Type) {
            Hit::Item(sym) if sym.kind == SymbolKind::Type => {
                self.add(EdgeKind::References, sym.id.clone());
                for field in &s.fields {
                    let Member::Named(name) = &field.member else {
                        continue;
                    };
                    if let Some(found) = self.idx.field(&sym, &name.to_string()) {
                        let id = found.id.clone();
                        self.add(EdgeKind::References, id);
                    }
                }
            }
            Hit::Item(sym) if sym.kind == SymbolKind::Field => {
                self.add(EdgeKind::References, sym.id.clone());
            }
            _ => {}
        }
        visit::visit_expr_struct(self, s);
    }

    fn field_access(&mut self, f: &syn::ExprField) {
        if let (Some(owner), Member::Named(name)) = (self.expr_hint(&f.base), &f.member)
            && let Some(found) = self.idx.field(&owner, &name.to_string())
        {
            let id = found.id.clone();
            self.add(EdgeKind::References, id);
        }
        visit::visit_expr_field(self, f);
    }

    /// The callee when the body is one call that passes the receiver and every
    /// parameter on, in order.
    fn forwards(&self, sig: &syn::Signature, block: &Block) -> Option<String> {
        let [Stmt::Expr(expr, _)] = block.stmts.as_slice() else {
            return None;
        };
        let expr = match expr {
            Expr::Return(r) => r.expr.as_deref()?,
            other => other,
        };
        let names = parameter_names(sig)?;
        let receiver = has_receiver(sig);
        match expr {
            Expr::MethodCall(m) if receiver => {
                if !is_self(&m.receiver) || !same_names(&names, m.args.iter(), 0) {
                    return None;
                }
                let recv = self.cx.self_ty.clone()?;
                self.idx.method(&recv, &m.method.to_string())
            }
            Expr::Call(c) => {
                let skip = usize::from(receiver);
                if receiver && !c.args.first().is_some_and(is_self) {
                    return None;
                }
                if !same_names(&names, c.args.iter().skip(skip), 0) {
                    return None;
                }
                let Expr::Path(p) = &*c.func else {
                    return None;
                };
                let hit = self.hit(&p.path, Ns::Value);
                match self.call_target(&hit)? {
                    (EdgeKind::Calls, id) => Some(id),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Walks `block` and summarizes the calls, flow and counts of the body.
pub fn analyze(idx: &Index, home: &Home, sig: &syn::Signature, block: &Block) -> Facts {
    let mut walker = Walker::new(idx, home, sig);
    walker.mark_tail(block);
    walker.bind_params(sig);
    walker.visit_block(block);
    let forwards_to = walker.forwards(sig, block);
    Facts {
        uses: walker.uses,
        unread_macros: walker.unread_macros,
        flow: walker.flow,
        max_nesting: walker.deepest,
        statements: walker.count,
        nested: walker.nested,
        forwards_to,
    }
}

fn has_receiver(sig: &syn::Signature) -> bool {
    sig.receiver().is_some()
}

fn is_default(arm: &syn::Arm) -> bool {
    if arm.guard.is_some() {
        return false;
    }
    match &arm.pat {
        Pat::Wild(_) => true,
        Pat::Ident(i) => i.subpat.is_none() && !i.ident.to_string().starts_with(char::is_uppercase),
        _ => false,
    }
}

fn is_return(body: &Expr) -> bool {
    match body {
        Expr::Return(_) => true,
        Expr::Block(b) => matches!(b.block.stmts.as_slice(), [Stmt::Expr(e, _)] if is_return(e)),
        _ => false,
    }
}

/// An arm body that is one value or `return`, with no statements of its own.
fn single_value(body: &Expr) -> bool {
    match body {
        Expr::Block(b) => match b.block.stmts.as_slice() {
            [Stmt::Expr(e, _)] => single_value(e),
            [Stmt::Macro(_)] => true,
            _ => false,
        },
        Expr::If(_)
        | Expr::Match(_)
        | Expr::ForLoop(_)
        | Expr::While(_)
        | Expr::Loop(_)
        | Expr::Break(_)
        | Expr::Continue(_) => false,
        _ => true,
    }
}

fn flatten_logic<'e>(e: &'e Expr, ops: &mut Vec<BinOp>, leaves: &mut Vec<&'e Expr>) {
    match peel(e) {
        Expr::Binary(b) if is_logic(&b.op) => {
            flatten_logic(&b.left, ops, leaves);
            ops.push(b.op);
            flatten_logic(&b.right, ops, leaves);
        }
        leaf => leaves.push(leaf),
    }
}

fn peel(mut e: &Expr) -> &Expr {
    loop {
        match e {
            Expr::Paren(p) => e = &p.expr,
            Expr::Group(g) => e = &g.expr,
            _ => return e,
        }
    }
}

fn is_logic(op: &BinOp) -> bool {
    matches!(op, BinOp::And(_) | BinOp::Or(_))
}

fn is_self(e: &Expr) -> bool {
    matches!(peel(e), Expr::Path(p) if p.path.is_ident("self"))
}

/// Parameter names in order; `None` when a parameter is not a plain name.
fn parameter_names(sig: &syn::Signature) -> Option<Vec<String>> {
    sig.inputs
        .iter()
        .filter_map(|a| match a {
            syn::FnArg::Typed(t) => Some(t),
            syn::FnArg::Receiver(_) => None,
        })
        .map(|t| match &*t.pat {
            Pat::Ident(i) if i.ident != "_" && i.subpat.is_none() => Some(i.ident.to_string()),
            _ => None,
        })
        .collect()
}

fn same_names<'e>(names: &[String], args: impl Iterator<Item = &'e Expr>, skip: usize) -> bool {
    let args: Vec<&Expr> = args.skip(skip).collect();
    args.len() == names.len()
        && args
            .iter()
            .zip(names)
            .all(|(a, n)| matches!(a, Expr::Path(p) if p.path.is_ident(n)))
}
