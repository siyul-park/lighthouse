use super::*;

pub(super) fn impl_item(
    file: &SourceFile,
    pointer: &str,
    value: &ImplItem,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    let kind = match value {
        ImplItem::Const(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &value.ident, source_map)?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &value.ty, source_map)?,
            );
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &value.expr, source_map)?,
            );
            "ImplItemConst"
        }
        ImplItem::Fn(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &value.vis, source_map)?,
            );
            fields.insert(
                "sig".into(),
                signature(file, &child(pointer, "sig"), &value.sig, source_map)?,
            );
            fields.insert(
                "block".into(),
                block(file, &child(pointer, "block"), &value.block, source_map)?,
            );
            "ImplItemFn"
        }
        ImplItem::Type(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &value.ident, source_map)?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &value.ty, source_map)?,
            );
            "ImplItemType"
        }
        ImplItem::Macro(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "mac".into(),
                macro_ast(file, &child(pointer, "mac"), &value.mac, source_map)?,
            );
            "ImplItemMacro"
        }
        ImplItem::Verbatim(tokens) => {
            fields.insert("tokens".into(), Value::String(tokens.to_string()));
            "ImplItemVerbatim"
        }
        _ => {
            fields.insert(
                "tokens".into(),
                Value::String(value.to_token_stream().to_string()),
            );
            "ImplItemOther"
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

pub(super) fn trait_item(
    file: &SourceFile,
    pointer: &str,
    value: &syn::TraitItem,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    let kind = match value {
        syn::TraitItem::Const(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &value.ident, source_map)?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &value.ty, source_map)?,
            );
            fields.insert(
                "default".into(),
                value
                    .default
                    .as_ref()
                    .map_or(Ok(Value::Null), |(_, expr)| {
                        expr_ast(file, &child(pointer, "default"), expr, source_map)
                    })?,
            );
            "TraitItemConst"
        }
        syn::TraitItem::Fn(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "sig".into(),
                signature(file, &child(pointer, "sig"), &value.sig, source_map)?,
            );
            fields.insert(
                "default".into(),
                value.default.as_ref().map_or(Ok(Value::Null), |block| {
                    block_ast(file, &child(pointer, "default"), block, source_map)
                })?,
            );
            "TraitItemFn"
        }
        syn::TraitItem::Type(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &value.ident, source_map)?,
            );
            fields.insert(
                "bounds".into(),
                token_value(file, &child(pointer, "bounds"), &value.bounds, source_map)?,
            );
            fields.insert(
                "default".into(),
                value.default.as_ref().map_or(Ok(Value::Null), |(_, ty)| {
                    type_ast(file, &child(pointer, "default"), ty, source_map)
                })?,
            );
            "TraitItemType"
        }
        syn::TraitItem::Macro(value) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &value.attrs, source_map)?,
            );
            fields.insert(
                "mac".into(),
                macro_ast(file, &child(pointer, "mac"), &value.mac, source_map)?,
            );
            "TraitItemMacro"
        }
        syn::TraitItem::Verbatim(tokens) => {
            fields.insert("tokens".into(), Value::String(tokens.to_string()));
            "TraitItemVerbatim"
        }
        _ => {
            fields.insert(
                "tokens".into(),
                Value::String(value.to_token_stream().to_string()),
            );
            "TraitItemOther"
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
