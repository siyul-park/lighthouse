use super::*;

pub(super) fn item_ast(
    file: &SourceFile,
    pointer: &str,
    item: &Item,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    match item {
        Item::Const(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &item.ty, source_map)?,
            );
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &item.expr, source_map)?,
            );
        }
        Item::Enum(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            fields.insert(
                "variants".into(),
                variants(
                    file,
                    &child(pointer, "variants"),
                    &item.variants,
                    source_map,
                )?,
            );
        }
        Item::ExternCrate(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            let rename = match &item.rename {
                Some((_, ident)) => ident_ast(file, &child(pointer, "rename"), ident, source_map)?,
                None => Value::Null,
            };
            fields.insert("rename".into(), rename);
        }
        Item::Fn(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "sig".into(),
                signature(file, &child(pointer, "sig"), &item.sig, source_map)?,
            );
            fields.insert(
                "block".into(),
                block(file, &child(pointer, "block"), &item.block, source_map)?,
            );
        }
        Item::ForeignMod(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "abi".into(),
                token_value(file, &child(pointer, "abi"), &item.abi, source_map)?,
            );
            fields.insert(
                "items".into(),
                array_tokens(file, &child(pointer, "items"), &item.items, source_map)?,
            );
        }
        Item::Impl(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            let trait_ = match &item.trait_ {
                Some((_, path, _)) => path_ast(file, &child(pointer, "trait"), path, source_map)?,
                None => Value::Null,
            };
            fields.insert("trait".into(), trait_);
            fields.insert(
                "self_ty".into(),
                type_ast(file, &child(pointer, "self_ty"), &item.self_ty, source_map)?,
            );
            let mut members = Vec::with_capacity(item.items.len());
            for (index, member) in item.items.iter().enumerate() {
                members.push(impl_item(
                    file,
                    &pointer_child(&child(pointer, "items"), &index.to_string()),
                    member,
                    source_map,
                )?);
            }
            fields.insert("items".into(), Value::Array(members));
        }
        Item::Macro(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            let ident = match &item.ident {
                Some(ident) => ident_ast(file, &child(pointer, "ident"), ident, source_map)?,
                None => Value::Null,
            };
            fields.insert("ident".into(), ident);
            fields.insert(
                "mac".into(),
                macro_ast(file, &child(pointer, "mac"), &item.mac, source_map)?,
            );
        }
        Item::Mod(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            if let Some((_, items)) = &item.content {
                let mut nested = Vec::with_capacity(items.len());
                for (index, nested_item) in items.iter().enumerate() {
                    nested.push(item_ast(
                        file,
                        &pointer_child(&child(pointer, "items"), &index.to_string()),
                        nested_item,
                        source_map,
                    )?);
                }
                fields.insert("items".into(), Value::Array(nested));
            } else {
                fields.insert("items".into(), Value::Null);
            }
        }
        Item::Static(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "mutability".into(),
                token_value(
                    file,
                    &child(pointer, "mutability"),
                    &item.mutability,
                    source_map,
                )?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &item.ty, source_map)?,
            );
            fields.insert(
                "expr".into(),
                expr_ast(file, &child(pointer, "expr"), &item.expr, source_map)?,
            );
        }
        Item::Struct(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            fields.insert(
                "fields".into(),
                fields_ast(file, &child(pointer, "fields"), &item.fields, source_map)?,
            );
        }
        Item::Trait(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            fields.insert(
                "supertraits".into(),
                token_value(
                    file,
                    &child(pointer, "supertraits"),
                    &item.supertraits,
                    source_map,
                )?,
            );
            let mut members = Vec::with_capacity(item.items.len());
            for (index, member) in item.items.iter().enumerate() {
                members.push(trait_item(
                    file,
                    &pointer_child(&child(pointer, "items"), &index.to_string()),
                    member,
                    source_map,
                )?);
            }
            fields.insert("items".into(), Value::Array(members));
        }
        Item::TraitAlias(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            fields.insert(
                "bounds".into(),
                token_value(file, &child(pointer, "bounds"), &item.bounds, source_map)?,
            );
        }
        Item::Type(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &item.ty, source_map)?,
            );
        }
        Item::Union(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &item.ident, source_map)?,
            );
            fields.insert(
                "generics".into(),
                token_value(
                    file,
                    &child(pointer, "generics"),
                    &item.generics,
                    source_map,
                )?,
            );
            fields.insert(
                "fields".into(),
                fields_ast(
                    file,
                    &child(pointer, "fields"),
                    &Fields::Named(item.fields.clone()),
                    source_map,
                )?,
            );
        }
        Item::Use(item) => {
            fields.insert(
                "attrs".into(),
                attributes(file, &child(pointer, "attrs"), &item.attrs, source_map)?,
            );
            fields.insert(
                "vis".into(),
                visibility(file, &child(pointer, "vis"), &item.vis, source_map)?,
            );
            fields.insert(
                "tree".into(),
                token_value(file, &child(pointer, "tree"), &item.tree, source_map)?,
            );
        }
        Item::Verbatim(tokens) => {
            fields.insert("tokens".into(), Value::String(tokens.to_string()));
        }
        _ => {
            fields.insert(
                "tokens".into(),
                Value::String(item.to_token_stream().to_string()),
            );
        }
    }
    Ok(node(
        file,
        pointer,
        item_kind(item),
        fields,
        item.span(),
        true,
        source_map,
    ))
}

pub(super) fn item_kind(item: &Item) -> &'static str {
    match item {
        Item::Const(_) => "ItemConst",
        Item::Enum(_) => "ItemEnum",
        Item::ExternCrate(_) => "ItemExternCrate",
        Item::Fn(_) => "ItemFn",
        Item::ForeignMod(_) => "ItemForeignMod",
        Item::Impl(_) => "ItemImpl",
        Item::Macro(_) => "ItemMacro",
        Item::Mod(_) => "ItemMod",
        Item::Static(_) => "ItemStatic",
        Item::Struct(_) => "ItemStruct",
        Item::Trait(_) => "ItemTrait",
        Item::TraitAlias(_) => "ItemTraitAlias",
        Item::Type(_) => "ItemType",
        Item::Union(_) => "ItemUnion",
        Item::Use(_) => "ItemUse",
        Item::Verbatim(_) => "ItemVerbatim",
        _ => "ItemUnknown",
    }
}

pub(super) fn variants(
    file: &SourceFile,
    pointer: &str,
    values: &syn::punctuated::Punctuated<Variant, syn::token::Comma>,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut output = Vec::with_capacity(values.len());
    for (index, variant) in values.iter().enumerate() {
        let at = pointer_child(pointer, &index.to_string());
        let mut fields = Map::new();
        fields.insert(
            "attrs".into(),
            attributes(file, &child(&at, "attrs"), &variant.attrs, source_map)?,
        );
        fields.insert(
            "ident".into(),
            ident(file, &child(&at, "ident"), &variant.ident, source_map)?,
        );
        fields.insert(
            "fields".into(),
            fields_ast(file, &child(&at, "fields"), &variant.fields, source_map)?,
        );
        fields.insert(
            "discriminant".into(),
            variant
                .discriminant
                .as_ref()
                .map_or(Ok(Value::Null), |(_, expr)| {
                    expr_ast(file, &child(&at, "discriminant"), expr, source_map)
                })?,
        );
        output.push(node(
            file,
            &at,
            "Variant",
            fields,
            variant.span(),
            false,
            source_map,
        ));
    }
    Ok(Value::Array(output))
}

pub(super) fn fields_ast(
    file: &SourceFile,
    pointer: &str,
    fields: &Fields,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let (kind, fields): (&str, Vec<syn::Field>) = match fields {
        Fields::Named(fields) => ("FieldsNamed", fields.named.iter().cloned().collect()),
        Fields::Unnamed(fields) => ("FieldsUnnamed", fields.unnamed.iter().cloned().collect()),
        Fields::Unit => ("FieldsUnit", Vec::new()),
    };
    let mut items = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let at = pointer_child(&child(pointer, "fields"), &index.to_string());
        let mut values = Map::new();
        values.insert(
            "attrs".into(),
            attributes(file, &child(&at, "attrs"), &field.attrs, source_map)?,
        );
        values.insert(
            "vis".into(),
            visibility(file, &child(&at, "vis"), &field.vis, source_map)?,
        );
        values.insert(
            "ident".into(),
            field.ident.as_ref().map_or(Ok(Value::Null), |ident| {
                ident_ast(file, &child(&at, "ident"), ident, source_map)
            })?,
        );
        values.insert(
            "ty".into(),
            type_ast(file, &child(&at, "ty"), &field.ty, source_map)?,
        );
        items.push(node(
            file,
            &at,
            "Field",
            values,
            field.span(),
            false,
            source_map,
        ));
    }
    let mut container = Map::new();
    container.insert("fields".into(), Value::Array(items));
    Ok(node(
        file,
        pointer,
        kind,
        container,
        fields_span(&fields).unwrap_or_else(|| fields_tokens_span(&fields)),
        false,
        source_map,
    ))
}

pub(super) fn fields_span(fields: &[syn::Field]) -> Option<proc_macro2::Span> {
    let first = fields.first()?.span();
    let last = fields.last()?.span();
    Some(first.join(last).unwrap_or(last))
}

pub(super) fn fields_tokens_span(fields: &[syn::Field]) -> proc_macro2::Span {
    fields
        .first()
        .map(Spanned::span)
        .unwrap_or_else(proc_macro2::Span::call_site)
}
