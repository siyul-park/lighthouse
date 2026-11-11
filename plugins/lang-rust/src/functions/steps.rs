//! The top-level statements of a body, each with the syntax counted inside it:
//! the walk bumps one tally and a step is what the tally gained over a
//! statement.

use lighthouse_protocol::{Step, StepCall, StepKind};

use super::{flow::only_jumps, *};

/// What the walk has counted so far.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) operators: u32,
    pub(super) indexing: u32,
    pub(super) literals: u32,
    pub(super) calls: Vec<StepCall>,
}

/// Where a [`Tally`] stood.
pub(super) struct Mark {
    operators: u32,
    indexing: u32,
    literals: u32,
    calls: usize,
}

/// What a [`Tally`] gained since a [`Mark`].
pub(super) struct Gain {
    operators: u32,
    indexing: u32,
    literals: u32,
    calls: Vec<StepCall>,
}

impl Walker<'_> {
    pub(super) fn tally_method(&mut self, m: &ExprMethodCall) {
        let call = StepCall {
            name: m.method.to_string(),
            site: ident_span(self.file, &m.method),
        };
        self.tally.calls.push(call);
    }

    pub(super) fn tally_macro(&mut self, m: &syn::Macro) {
        let Some(last) = m.path.segments.last() else {
            return;
        };
        if last.ident == "vec" {
            self.tally.literals += 1;
            return;
        }
        let call = StepCall {
            name: format!("{}!", last.ident),
            site: ident_span(self.file, &last.ident),
        };
        self.tally.calls.push(call);
    }

    /// A call of a path whose last segment starts with a capital is the
    /// construction of a tuple struct or variant, not a call.
    pub(super) fn tally_call(&mut self, c: &ExprCall) {
        let call = match &*c.func {
            Expr::Path(p) => {
                let Some(last) = p.path.segments.last() else {
                    return;
                };
                if last.ident.to_string().starts_with(char::is_uppercase) {
                    return;
                }
                let name: Vec<String> = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                StepCall {
                    name: name.join("::"),
                    site: ident_span(self.file, &last.ident),
                }
            }
            other => StepCall {
                name: String::new(),
                site: span_of(self.file, other),
            },
        };
        self.tally.calls.push(call);
    }

    /// Walks the body, one step to each of its statements.
    pub(super) fn visit_body(&mut self, block: &Block) {
        self.open(block);
        let last = block.stmts.len().saturating_sub(1);
        for (at, stmt) in block.stmts.iter().enumerate() {
            let (count, mark) = (self.count, self.tally.mark());
            self.visit_stmt(stmt);
            if matches!(stmt, Stmt::Item(_)) {
                continue;
            }
            let gain = self.tally.since(&mark);
            let step = Step {
                span: span_of(self.file, stmt),
                kind: step_kind(stmt, at == last),
                operators: gain.operators,
                indexing: gain.indexing,
                literals: gain.literals,
                calls: gain.calls,
                nested: self.count - count,
                guard: matches!(stmt, Stmt::Expr(Expr::If(i), _)
                    if i.else_branch.is_none() && only_jumps(&i.then_branch)),
            };
            self.steps.push(step);
        }
        self.scopes.pop();
    }
}

impl Tally {
    pub(super) fn mark(&self) -> Mark {
        Mark {
            operators: self.operators,
            indexing: self.indexing,
            literals: self.literals,
            calls: self.calls.len(),
        }
    }

    /// What was counted since `mark`, the calls in source order; they leave
    /// the tally.
    pub(super) fn since(&mut self, mark: &Mark) -> Gain {
        let mut calls = self.calls.split_off(mark.calls);
        calls.sort_by_key(|c| (c.site.start.line, c.site.start.col));
        Gain {
            operators: self.operators - mark.operators,
            indexing: self.indexing - mark.indexing,
            literals: self.literals - mark.literals,
            calls,
        }
    }
}

/// Arithmetic, bitwise and shift operators, plain or compound.
pub(super) fn counts_as_operator(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::Add(_)
            | BinOp::Sub(_)
            | BinOp::Mul(_)
            | BinOp::Div(_)
            | BinOp::Rem(_)
            | BinOp::BitXor(_)
            | BinOp::BitAnd(_)
            | BinOp::BitOr(_)
            | BinOp::Shl(_)
            | BinOp::Shr(_)
            | BinOp::AddAssign(_)
            | BinOp::SubAssign(_)
            | BinOp::MulAssign(_)
            | BinOp::DivAssign(_)
            | BinOp::RemAssign(_)
            | BinOp::BitXorAssign(_)
            | BinOp::BitAndAssign(_)
            | BinOp::BitOrAssign(_)
            | BinOp::ShlAssign(_)
            | BinOp::ShrAssign(_)
    )
}

fn step_kind(stmt: &Stmt, last: bool) -> StepKind {
    match stmt {
        Stmt::Local(l) if l.init.is_some() => StepKind::Assign,
        Stmt::Local(_) => StepKind::Declare,
        Stmt::Expr(e, semi) => match e {
            Expr::If(_) => StepKind::If,
            Expr::Match(_) => StepKind::Switch,
            Expr::ForLoop(_) | Expr::While(_) | Expr::Loop(_) => StepKind::Loop,
            Expr::Block(_) | Expr::Unsafe(_) => StepKind::Block,
            Expr::Return(_) => StepKind::Return,
            Expr::Break(_) | Expr::Continue(_) => StepKind::Jump,
            Expr::Assign(_) => StepKind::Assign,
            Expr::Binary(b) if is_compound(&b.op) => StepKind::Assign,
            _ if semi.is_none() && last => StepKind::Return,
            _ => StepKind::Call,
        },
        Stmt::Macro(m) if m.semi_token.is_none() && last => StepKind::Return,
        _ => StepKind::Call,
    }
}

fn is_compound(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::AddAssign(_)
            | BinOp::SubAssign(_)
            | BinOp::MulAssign(_)
            | BinOp::DivAssign(_)
            | BinOp::RemAssign(_)
            | BinOp::BitXorAssign(_)
            | BinOp::BitAndAssign(_)
            | BinOp::BitOrAssign(_)
            | BinOp::ShlAssign(_)
            | BinOp::ShrAssign(_)
    )
}
