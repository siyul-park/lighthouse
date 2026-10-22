//! The `cel.bind` macro of cel-go's bindings extension: `cel.bind(name, init,
//! body)` is `body` with `name` bound to the value of `init`, evaluated once.

use cel::{
    DeclarationError, Env,
    common::ast::{ComprehensionExpr, Expr, IdedExpr, ListExpr, LiteralValue},
    parser::{Macro, MacroExprHelper, ParseError},
};

/// Registers the macro on `env`.
pub(crate) fn install(env: &mut Env) -> Result<(), DeclarationError> {
    env.add_macro(Macro::receiver("bind", 3, expand))
}

/// Expands `cel.bind(name, init, body)` into a comprehension over nothing
/// whose accumulator is `name`, as cel-go does; any other `bind` call is left
/// as written.
fn expand(
    helper: &mut MacroExprHelper<'_>,
    target: &mut Option<IdedExpr>,
    args: &mut Vec<IdedExpr>,
) -> Result<Option<IdedExpr>, ParseError> {
    if !matches!(target.as_ref().map(|t| &t.expr), Some(Expr::Ident(name)) if name == "cel") {
        return Ok(None);
    }
    let Expr::Ident(name) = &args[0].expr else {
        return Err(helper.new_error(
            args[0].id,
            "cel.bind's first argument must be an identifier",
        ));
    };
    let name = name.clone();
    let (Some(body), Some(init)) = (args.pop(), args.pop()) else {
        return Ok(None);
    };
    let accumulator =
        |helper: &mut MacroExprHelper<'_>| helper.next_expr(Expr::Ident(name.clone()));
    let comprehension = ComprehensionExpr {
        iter_range: helper.next_expr(Expr::List(ListExpr::new(Vec::new()))),
        iter_var: "#unused".to_owned(),
        iter_var2: None,
        accu_var: name.clone(),
        accu_init: init,
        loop_cond: helper.next_expr(Expr::Literal(LiteralValue::Boolean(false.into()))),
        loop_step: accumulator(helper),
        result: body,
    };
    Ok(Some(
        helper.next_expr(Expr::Comprehension(Box::new(comprehension))),
    ))
}
