//! The size and shape of an expression, as the function summary a check reads.

use cel::common::ast::{
    CallExpr, ComprehensionExpr, EntryExpr, Expr, IdedEntryExpr, IdedExpr, LiteralValue,
};
use lighthouse_model::ucm::{
    Flow, FlowKind, FunctionSummary, Signature, SymbolId, Target, TypeRef,
};

/// The operators whose result CEL fixes to a boolean, whatever their operands.
const BOOLEAN_OPERATORS: [&str; 10] = [
    "_&&_", "_||_", "!_", "_==_", "_!=_", "_<_", "_<=_", "_>_", "_>=_", "@in",
];

#[derive(Default)]
pub(super) struct Metrics {
    nodes: u32,
    max_nesting: u32,
    flow: Vec<Flow>,
}

impl Metrics {
    pub(super) fn of(expr: &IdedExpr) -> Self {
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
        call: &CallExpr,
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

    fn walk_entries(&mut self, entries: &[IdedEntryExpr], nesting: u32) {
        for entry in entries {
            match &entry.expr {
                EntryExpr::StructField(field) => {
                    self.walk(&field.value, nesting, None);
                }
                EntryExpr::MapEntry(entry) => {
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

    pub(super) fn summary(
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
            configures: None,
        }
    }
}

/// How many tokens the source has: words, quoted strings, two-character
/// operators and single characters, whatever their encoding.
pub(super) fn token_count(source: &str) -> u32 {
    let chars: Vec<char> = source.chars().collect();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut at = 0;
    let mut count = 0u32;
    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() {
            at += 1;
            continue;
        }
        count += 1;
        if c == '\'' || c == '"' {
            at += 1;
            while at < chars.len() {
                if chars[at] == '\\' {
                    at = (at + 2).min(chars.len());
                } else if chars[at] == c {
                    at += 1;
                    break;
                } else {
                    at += 1;
                }
            }
        } else if word(c) {
            at += 1;
            while at < chars.len() && word(chars[at]) {
                at += 1;
            }
        } else if let Some(next) = chars.get(at + 1)
            && ["&&", "||", "==", "!=", "<=", ">=", "??", "?."]
                .iter()
                .any(|pair| pair.chars().eq([c, *next]))
        {
            at += 2;
        } else {
            at += 1;
        }
    }
    count
}

/// The type of an expression where its form fixes it, else `dyn`: a boolean
/// literal, or a comparison or logical operator at the root.
pub(super) fn result_type(expr: &IdedExpr) -> &'static str {
    let boolean = is_boolean_literal(expr)
        || matches!(&expr.expr, Expr::Call(call) if BOOLEAN_OPERATORS.contains(&call.func_name.as_str()));
    if boolean { "bool" } else { "dyn" }
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
