//! The control flow of a body: branches, loops, jumps and boolean chains.

use super::*;

impl Walker<'_> {
    pub(super) fn if_expr(&mut self, i: &ExprIf, chained: bool) {
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

    pub(super) fn match_expr(&mut self, m: &ExprMatch) {
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

    pub(super) fn closure(&mut self, c: &ExprClosure) {
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

    pub(super) fn logic(&mut self, root: &Expr) {
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

    pub(super) fn for_loop(&mut self, f: &syn::ExprForLoop) {
        self.event(FlowKind::Loop);
        self.visit_expr(&f.expr);
        self.scoped(|w| {
            w.bind(&f.pat, None);
            w.visit_pat(&f.pat);
            w.nested_in(|w| w.visit_block(&f.body));
        });
    }

    pub(super) fn while_loop(&mut self, l: &syn::ExprWhile) {
        self.event(FlowKind::Loop);
        self.scoped(|w| {
            w.visit_expr(&l.cond);
            w.nested_in(|w| w.visit_block(&l.body));
        });
    }

    pub(super) fn plain_loop(&mut self, l: &syn::ExprLoop) {
        self.event(FlowKind::Loop);
        self.nested_in(|w| w.visit_block(&l.body));
    }

    /// `break 'label` and `continue 'label` jump; plain ones do not.
    pub(super) fn jump(&mut self, labeled: bool, rest: impl FnOnce(&mut Self)) {
        if labeled {
            self.event(FlowKind::Jump);
        }
        rest(self);
    }

    /// `return match ...` puts the match in return position.
    pub(super) fn return_expr(&mut self, r: &syn::ExprReturn) {
        if let Some(Expr::Match(m)) = r.expr.as_deref() {
            self.tails.insert(std::ptr::from_ref(m));
        }
        visit::visit_expr_return(self, r);
    }

    pub(super) fn let_expr(&mut self, l: &syn::ExprLet) {
        let hint = self.expr_hint(&l.expr);
        self.visit_expr(&l.expr);
        self.visit_pat(&l.pat);
        self.bind(&l.pat, hint);
    }
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

pub(super) fn peel(mut e: &Expr) -> &Expr {
    loop {
        match e {
            Expr::Paren(p) => e = &p.expr,
            Expr::Group(g) => e = &g.expr,
            _ => return e,
        }
    }
}

pub(super) fn is_logic(op: &BinOp) -> bool {
    matches!(op, BinOp::And(_) | BinOp::Or(_))
}
