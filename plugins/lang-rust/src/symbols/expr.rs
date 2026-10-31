use super::*;

pub(super) fn block(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Block,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut statements = Vec::with_capacity(value.stmts.len());
    for (index, statement) in value.stmts.iter().enumerate() {
        statements.push(stmt_ast(
            file,
            &pointer_child(&child(pointer, "stmts"), &index.to_string()),
            statement,
            source_map,
        )?);
    }
    Ok(node(
        file,
        pointer,
        "Block",
        Map::from_iter([("stmts".to_owned(), Value::Array(statements))]),
        value.span(),
        true,
        source_map,
    ))
}

pub(super) fn stmt_ast(
    file: &SourceFile,
    pointer: &str,
    value: &Stmt,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    let (kind, span) = match value {
        Stmt::Local(local) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &local.attrs, source_map)?,
            );
            fields.insert(
                "pat".into(),
                pat_ast(file, &child(pointer, "pat"), &local.pat, source_map)?,
            );
            fields.insert(
                "init".into(),
                local.init.as_ref().map_or(Ok(Value::Null), |init| {
                    expr_ast(file, &child(pointer, "init"), &init.expr, source_map)
                })?,
            );
            ("Local", local.span())
        }
        Stmt::Item(item) => {
            fields.insert(
                "item".into(),
                item_ast(file, &child(pointer, "item"), item, source_map)?,
            );
            ("Item", item.span())
        }
        Stmt::Expr(expr, _) => {
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), expr, source_map)?,
            );
            ("Expr", expr.span())
        }
        Stmt::Macro(mac) => {
            fields.insert(
                "mac".into(),
                macro_ast(file, &child(pointer, "mac"), &mac.mac, source_map)?,
            );
            ("Macro", mac.span())
        }
    };
    Ok(node(file, pointer, kind, fields, span, true, source_map))
}

pub(super) fn expr_ast(
    file: &SourceFile,
    pointer: &str,
    value: &Expr,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    let kind = match value {
        Expr::Array(value) => {
            fields.insert(
                "elems".into(),
                exprs(file, &child(pointer, "elems"), &value.elems, source_map)?,
            );
            "Array"
        }
        Expr::Assign(value) => {
            fields.insert(
                "left".into(),
                expr_ast(file, &child(pointer, "left"), &value.left, source_map)?,
            );
            fields.insert(
                "right".into(),
                expr_ast(file, &child(pointer, "right"), &value.right, source_map)?,
            );
            "Assign"
        }
        Expr::Async(value) => {
            fields.insert(
                "block".into(),
                block(file, &child(pointer, "block"), &value.block, source_map)?,
            );
            "Async"
        }
        Expr::Await(value) => {
            fields.insert(
                "base".into(),
                expr_ast(file, &child(pointer, "base"), &value.base, source_map)?,
            );
            "Await"
        }
        Expr::Binary(value) => {
            fields.insert(
                "left".into(),
                expr_ast(file, &child(pointer, "left"), &value.left, source_map)?,
            );
            fields.insert(
                "op".into(),
                Value::String(value.op.to_token_stream().to_string()),
            );
            fields.insert(
                "right".into(),
                expr_ast(file, &child(pointer, "right"), &value.right, source_map)?,
            );
            "Binary"
        }
        Expr::Block(value) => {
            fields.insert(
                "label".into(),
                token_value(file, &child(pointer, "label"), &value.label, source_map)?,
            );
            fields.insert(
                "block".into(),
                block(file, &child(pointer, "block"), &value.block, source_map)?,
            );
            "Block"
        }
        Expr::Break(value) => {
            fields.insert(
                "label".into(),
                token_value(file, &child(pointer, "label"), &value.label, source_map)?,
            );
            fields.insert(
                "expr".into(),
                value.expr.as_ref().map_or(Ok(Value::Null), |expr| {
                    expr_ast(file, &child(pointer, "expr"), expr, source_map)
                })?,
            );
            "Break"
        }
        Expr::Call(value) => {
            fields.insert(
                "func".into(),
                expr_ast(file, &child(pointer, "func"), &value.func, source_map)?,
            );
            fields.insert(
                "args".into(),
                exprs(file, &child(pointer, "args"), &value.args, source_map)?,
            );
            "Call"
        }
        Expr::Cast(value) => {
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &value.ty, source_map)?,
            );
            "Cast"
        }
        Expr::Closure(value) => {
            let mut inputs = Vec::with_capacity(value.inputs.len());
            for (index, input) in value.inputs.iter().enumerate() {
                inputs.push(pat_ast(
                    file,
                    &pointer_child(&child(pointer, "inputs"), &index.to_string()),
                    input,
                    source_map,
                )?);
            }
            fields.insert("inputs".into(), Value::Array(inputs));
            fields.insert(
                "output".into(),
                token_value(file, &child(pointer, "output"), &value.output, source_map)?,
            );
            fields.insert(
                "body".into(),
                expr_ast(file, &child(pointer, "body"), &value.body, source_map)?,
            );
            "Closure"
        }
        Expr::Field(value) => {
            fields.insert(
                "base".into(),
                expr_ast(file, &child(pointer, "base"), &value.base, source_map)?,
            );
            fields.insert(
                "member".into(),
                Value::String(value.member.to_token_stream().to_string()),
            );
            "Field"
        }
        Expr::ForLoop(value) => {
            fields.insert(
                "pat".into(),
                pat_ast(file, &child(pointer, "pat"), &value.pat, source_map)?,
            );
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            fields.insert(
                "body".into(),
                block(file, &child(pointer, "body"), &value.body, source_map)?,
            );
            "ForLoop"
        }
        Expr::Group(value) => {
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            "Group"
        }
        Expr::If(value) => {
            fields.insert(
                "cond".into(),
                expr_ast(file, &child(pointer, "cond"), &value.cond, source_map)?,
            );
            fields.insert(
                "then_branch".into(),
                block(
                    file,
                    &child(pointer, "then_branch"),
                    &value.then_branch,
                    source_map,
                )?,
            );
            fields.insert(
                "else_branch".into(),
                value
                    .else_branch
                    .as_ref()
                    .map_or(Ok(Value::Null), |(_, expr)| {
                        expr_ast(file, &child(pointer, "else_branch"), expr, source_map)
                    })?,
            );
            "If"
        }
        Expr::Index(value) => {
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            fields.insert(
                "index".into(),
                expr_ast(file, &child(pointer, "index"), &value.index, source_map)?,
            );
            "Index"
        }
        Expr::Lit(value) => {
            fields.insert(
                "lit".into(),
                Value::String(value.lit.to_token_stream().to_string()),
            );
            "Lit"
        }
        Expr::Loop(value) => {
            fields.insert(
                "body".into(),
                block(file, &child(pointer, "body"), &value.body, source_map)?,
            );
            "Loop"
        }
        Expr::Macro(value) => {
            fields.insert(
                "mac".into(),
                macro_ast(file, &child(pointer, "mac"), &value.mac, source_map)?,
            );
            "Macro"
        }
        Expr::Match(value) => {
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            let mut arms = Vec::with_capacity(value.arms.len());
            for (index, arm) in value.arms.iter().enumerate() {
                let at = pointer_child(&child(pointer, "arms"), &index.to_string());
                let mut arm_fields = Map::new();
                arm_fields.insert(
                    "pat".into(),
                    pat_ast(file, &child(&at, "pat"), &arm.pat, source_map)?,
                );
                arm_fields.insert(
                    "guard".into(),
                    arm.guard.as_ref().map_or(Ok(Value::Null), |(_, expr)| {
                        expr_ast(file, &child(&at, "guard"), expr, source_map)
                    })?,
                );
                arm_fields.insert(
                    "body".into(),
                    expr_ast(file, &child(&at, "body"), &arm.body, source_map)?,
                );
                arms.push(node(
                    file,
                    &at,
                    "Arm",
                    arm_fields,
                    arm.span(),
                    true,
                    source_map,
                ));
            }
            fields.insert("arms".into(), Value::Array(arms));
            "Match"
        }
        Expr::MethodCall(value) => {
            fields.insert(
                "receiver".into(),
                expr_ast(
                    file,
                    &child(pointer, "receiver"),
                    &value.receiver,
                    source_map,
                )?,
            );
            fields.insert(
                "method".into(),
                ident(file, &child(pointer, "method"), &value.method, source_map)?,
            );
            fields.insert(
                "args".into(),
                exprs(file, &child(pointer, "args"), &value.args, source_map)?,
            );
            "MethodCall"
        }
        Expr::Paren(value) => {
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            "Paren"
        }
        Expr::Path(value) => {
            fields.insert(
                "qself".into(),
                qself(file, &child(pointer, "qself"), &value.qself, source_map)?,
            );
            fields.insert(
                "path".into(),
                path_ast(file, &child(pointer, "path"), &value.path, source_map)?,
            );
            "Path"
        }
        Expr::Reference(value) => {
            fields.insert(
                "mutability".into(),
                token_value(
                    file,
                    &child(pointer, "mutability"),
                    &value.mutability,
                    source_map,
                )?,
            );
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            "Reference"
        }
        Expr::Return(value) => {
            fields.insert(
                "expr".into(),
                value.expr.as_ref().map_or(Ok(Value::Null), |expr| {
                    expr_ast(file, &child(pointer, "expr"), expr, source_map)
                })?,
            );
            "Return"
        }
        Expr::Struct(value) => {
            fields.insert(
                "path".into(),
                path_ast(file, &child(pointer, "path"), &value.path, source_map)?,
            );
            let mut members = Vec::with_capacity(value.fields.len());
            for (index, field) in value.fields.iter().enumerate() {
                let at = pointer_child(&child(pointer, "fields"), &index.to_string());
                let mut field_values = Map::new();
                field_values.insert(
                    "member".into(),
                    token_value(file, &child(&at, "member"), &field.member, source_map)?,
                );
                field_values.insert(
                    "expr".into(),
                    expr_ast(file, &child(&at, "expr"), &field.expr, source_map)?,
                );
                members.push(node(
                    file,
                    &at,
                    "FieldValue",
                    field_values,
                    field.span(),
                    false,
                    source_map,
                ));
            }
            fields.insert("fields".into(), Value::Array(members));
            fields.insert(
                "rest".into(),
                value.rest.as_ref().map_or(Ok(Value::Null), |expr| {
                    expr_ast(file, &child(pointer, "rest"), expr, source_map)
                })?,
            );
            "Struct"
        }
        Expr::Tuple(value) => {
            fields.insert(
                "elems".into(),
                exprs(file, &child(pointer, "elems"), &value.elems, source_map)?,
            );
            "Tuple"
        }
        Expr::Unary(value) => {
            fields.insert(
                "op".into(),
                Value::String(value.op.to_token_stream().to_string()),
            );
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            "Unary"
        }
        Expr::Unsafe(value) => {
            fields.insert(
                "block".into(),
                block(file, &child(pointer, "block"), &value.block, source_map)?,
            );
            "Unsafe"
        }
        Expr::While(value) => {
            fields.insert(
                "cond".into(),
                expr_ast(file, &child(pointer, "cond"), &value.cond, source_map)?,
            );
            fields.insert(
                "body".into(),
                block(file, &child(pointer, "body"), &value.body, source_map)?,
            );
            "While"
        }
        _ => {
            fields.insert(
                "tokens".into(),
                Value::String(value.to_token_stream().to_string()),
            );
            "ExprOther"
        }
    };
    Ok(node(
        file,
        pointer,
        kind,
        fields,
        value.span(),
        true,
        source_map,
    ))
}

pub(super) fn exprs(
    file: &SourceFile,
    pointer: &str,
    values: &syn::punctuated::Punctuated<Expr, syn::token::Comma>,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut output = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        output.push(expr_ast(
            file,
            &pointer_child(pointer, &index.to_string()),
            value,
            source_map,
        )?);
    }
    Ok(Value::Array(output))
}

pub(super) fn block_ast(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Block,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    block(file, pointer, value, source_map)
}
