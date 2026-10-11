//! What a call, a path or a field access denotes: the receiver types that are
//! syntactically known and the targets they lead to.

use super::{flow::peel, *};

impl Walker<'_> {
    pub(super) fn expr_hint(&self, e: &Expr) -> Option<Sym> {
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
                SymbolKind::Variant if !unwrap => self.idx.variant_owner.get(&sym.id).cloned(),
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
            Hit::Item(sym) if sym.kind == SymbolKind::Variant => {
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
}

impl Walker<'_> {
    pub(super) fn call(&mut self, c: &ExprCall) {
        if let Expr::Path(p) = &*c.func {
            let hit = self.hit(&p.path, Ns::Value);
            if let Some((kind, id)) = self.call_target(&hit) {
                let recursive = kind == EdgeKind::Calls && id == self.id;
                if let Some(at) = last_ident(&p.path) {
                    self.add(kind, id, at);
                }
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
                SymbolKind::Type | SymbolKind::Variant | SymbolKind::Const | SymbolKind::Var => {
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

    pub(super) fn method_call(&mut self, m: &ExprMethodCall) {
        let name = m.method.to_string();
        match self.expr_hint(&m.receiver) {
            Some(recv) => {
                if let Some(id) = self.idx.method(&recv, &name) {
                    let recursive = id == self.id;
                    self.add(EdgeKind::Calls, id, &m.method);
                    if recursive {
                        self.event(FlowKind::Recursion);
                    }
                }
            }
            None => self.may_reference(&m.method),
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
    fn may_reference(&mut self, at: &syn::Ident) {
        let name = at.to_string();
        let name = name.as_str();
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
                self.record(EdgeKind::References, id, Resolution::Heuristic, at);
            }
        }
    }

    pub(super) fn path_value(&mut self, p: &ExprPath) {
        if p.qself.is_none() {
            let hit = self.hit(&p.path, Ns::Value);
            if let Some((kind, id)) = self.value_target(&hit)
                && let Some(at) = last_ident(&p.path)
            {
                self.add(kind, id, at);
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

    pub(super) fn struct_literal(&mut self, s: &ExprStruct) {
        match self.hit(&s.path, Ns::Type) {
            Hit::Item(sym) if sym.kind == SymbolKind::Type => {
                if let Some(at) = last_ident(&s.path) {
                    self.add(EdgeKind::References, sym.id.clone(), at);
                }
                for field in &s.fields {
                    let Member::Named(name) = &field.member else {
                        continue;
                    };
                    if let Some(found) = self.idx.field(&sym, &name.to_string()) {
                        let id = found.id.clone();
                        self.add(EdgeKind::References, id, name);
                    }
                }
            }
            Hit::Item(sym) if sym.kind == SymbolKind::Variant => {
                if let Some(at) = last_ident(&s.path) {
                    self.add(EdgeKind::References, sym.id.clone(), at);
                }
            }
            _ => {}
        }
        visit::visit_expr_struct(self, s);
    }

    pub(super) fn field_access(&mut self, f: &syn::ExprField) {
        if let (Some(owner), Member::Named(name)) = (self.expr_hint(&f.base), &f.member)
            && let Some(found) = self.idx.field(&owner, &name.to_string())
        {
            let id = found.id.clone();
            self.add(EdgeKind::References, id, name);
        }
        visit::visit_expr_field(self, f);
    }

    /// The callee when the body is one call that passes the receiver and every
    /// parameter on, in order.
    pub(super) fn forwards(&self, sig: &syn::Signature, block: &Block) -> Option<String> {
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

fn has_receiver(sig: &syn::Signature) -> bool {
    sig.receiver().is_some()
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
