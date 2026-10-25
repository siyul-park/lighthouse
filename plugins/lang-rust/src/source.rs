use std::collections::BTreeMap;

use lighthouse_protocol::{Mapping, Source, TextMapSegment, pointer_child};
use quote::ToTokens;
use serde_json::{Map, Value};
use syn::{Expr, Fields, FnArg, ImplItem, Item, Pat, Stmt, Type, Variant, spanned::Spanned};

use crate::tree::SourceFile;

pub fn from_file(file: &SourceFile) -> Result<Source, String> {
    let mut source_map = BTreeMap::from([(
        String::new(),
        Mapping {
            range: Some([0, offset(file.text.len())?]),
            text_pointer: None,
            text_map: None,
        },
    )]);
    let mut root = Map::new();
    root.insert("kind".to_owned(), Value::String("File".to_owned()));
    root.insert(
        "shebang".to_owned(),
        file.ast.shebang.clone().map_or(Value::Null, Value::String),
    );
    root.insert(
        "attrs".to_owned(),
        attributes(file, "/attrs", &file.ast.attrs, &mut source_map)?,
    );
    let mut items = Vec::with_capacity(file.ast.items.len());
    for (index, item) in file.ast.items.iter().enumerate() {
        let pointer = pointer_child("/items", &index.to_string());
        items.push(item_ast(file, &pointer, item, &mut source_map)?);
    }
    root.insert("items".to_owned(), Value::Array(items));
    Ok(Source {
        path: file.rel.clone(),
        ast: Value::Object(root),
        mapping: source_map,
    })
}

fn item_ast(
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
            fields.insert(
                "rename".into(),
                item.rename.as_ref().map_or(Value::Null, |(_, ident)| {
                    ident_ast(file, &child(pointer, "rename"), ident, source_map)
                        .unwrap_or(Value::Null)
                }),
            );
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
            fields.insert(
                "trait".into(),
                item.trait_.as_ref().map_or(Value::Null, |(_, path, _)| {
                    path_ast(file, &child(pointer, "trait"), path, source_map)
                        .unwrap_or(Value::Null)
                }),
            );
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
            fields.insert(
                "ident".into(),
                item.ident.as_ref().map_or(Value::Null, |ident| {
                    ident_ast(file, &child(pointer, "ident"), ident, source_map)
                        .unwrap_or(Value::Null)
                }),
            );
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

fn item_kind(item: &Item) -> &'static str {
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

fn variants(
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

fn fields_ast(
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

fn fields_span(fields: &[syn::Field]) -> Option<proc_macro2::Span> {
    let first = fields.first()?.span();
    let last = fields.last()?.span();
    Some(first.join(last).unwrap_or(last))
}

fn fields_tokens_span(fields: &[syn::Field]) -> proc_macro2::Span {
    fields
        .first()
        .map(Spanned::span)
        .unwrap_or_else(proc_macro2::Span::call_site)
}

fn receiver_reference(
    file: &SourceFile,
    pointer: &str,
    value: &Option<(syn::token::And, Option<syn::Lifetime>)>,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    match value {
        None => {
            fields.insert("present".into(), Value::Bool(false));
        }
        Some((and, lifetime)) => {
            fields.insert("present".into(), Value::Bool(true));
            fields.insert(
                "ampersand".into(),
                token_value(file, &child(pointer, "ampersand"), and, source_map)?,
            );
            fields.insert(
                "lifetime".into(),
                lifetime.as_ref().map_or(Ok(Value::Null), |lifetime| {
                    token_value(file, &child(pointer, "lifetime"), lifetime, source_map)
                })?,
            );
        }
    }
    Ok(Value::Object(fields))
}

fn qself(
    file: &SourceFile,
    pointer: &str,
    value: &Option<syn::QSelf>,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let Some(value) = value else {
        return Ok(Value::Null);
    };
    let mut fields = Map::new();
    fields.insert("position".into(), Value::from(value.position));
    fields.insert(
        "as_token".into(),
        token_value(
            file,
            &child(pointer, "as_token"),
            &value.as_token,
            source_map,
        )?,
    );
    fields.insert(
        "ty".into(),
        type_ast(file, &child(pointer, "ty"), &value.ty, source_map)?,
    );
    Ok(node(
        file,
        pointer,
        "QSelf",
        fields,
        value.span(),
        false,
        source_map,
    ))
}

fn signature(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Signature,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    fields.insert(
        "ident".into(),
        ident(file, &child(pointer, "ident"), &value.ident, source_map)?,
    );
    fields.insert(
        "generics".into(),
        token_value(
            file,
            &child(pointer, "generics"),
            &value.generics,
            source_map,
        )?,
    );
    let mut inputs = Vec::with_capacity(value.inputs.len());
    for (index, input) in value.inputs.iter().enumerate() {
        let at = pointer_child(&child(pointer, "inputs"), &index.to_string());
        inputs.push(match input {
            FnArg::Receiver(receiver) => {
                let mut fields = Map::new();
                fields.insert(
                    "attrs".into(),
                    attributes(file, &child(&at, "attrs"), &receiver.attrs, source_map)?,
                );
                fields.insert(
                    "reference".into(),
                    receiver_reference(
                        file,
                        &child(&at, "reference"),
                        &receiver.reference,
                        source_map,
                    )?,
                );
                fields.insert(
                    "mutability".into(),
                    token_value(
                        file,
                        &child(&at, "mutability"),
                        &receiver.mutability,
                        source_map,
                    )?,
                );
                fields.insert(
                    "ty".into(),
                    type_ast(file, &child(&at, "ty"), &receiver.ty, source_map)?,
                );
                node(
                    file,
                    &at,
                    "Receiver",
                    fields,
                    receiver.span(),
                    false,
                    source_map,
                )
            }
            FnArg::Typed(typed) => {
                let mut fields = Map::new();
                fields.insert(
                    "attrs".into(),
                    attributes(file, &child(&at, "attrs"), &typed.attrs, source_map)?,
                );
                fields.insert(
                    "pat".into(),
                    pat_ast(file, &child(&at, "pat"), &typed.pat, source_map)?,
                );
                fields.insert(
                    "ty".into(),
                    type_ast(file, &child(&at, "ty"), &typed.ty, source_map)?,
                );
                node(
                    file,
                    &at,
                    "TypedArgument",
                    fields,
                    typed.span(),
                    false,
                    source_map,
                )
            }
        });
    }
    fields.insert("inputs".into(), Value::Array(inputs));
    fields.insert(
        "output".into(),
        token_value(file, &child(pointer, "output"), &value.output, source_map)?,
    );
    fields.insert(
        "asyncness".into(),
        token_value(
            file,
            &child(pointer, "asyncness"),
            &value.asyncness,
            source_map,
        )?,
    );
    fields.insert(
        "constness".into(),
        token_value(
            file,
            &child(pointer, "constness"),
            &value.constness,
            source_map,
        )?,
    );
    fields.insert(
        "unsafety".into(),
        token_value(
            file,
            &child(pointer, "unsafety"),
            &value.unsafety,
            source_map,
        )?,
    );
    fields.insert(
        "abi".into(),
        token_value(file, &child(pointer, "abi"), &value.abi, source_map)?,
    );
    Ok(node(
        file,
        pointer,
        "Signature",
        fields,
        value.span(),
        false,
        source_map,
    ))
}

fn block(
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

fn stmt_ast(
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

fn expr_ast(
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

fn exprs(
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

fn type_ast(
    file: &SourceFile,
    pointer: &str,
    value: &Type,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    let kind = match value {
        Type::Array(value) => {
            fields.insert(
                "elem".into(),
                type_ast(file, &child(pointer, "elem"), &value.elem, source_map)?,
            );
            fields.insert(
                "len".into(),
                expr_ast(file, &child(pointer, "len"), &value.len, source_map)?,
            );
            "Array"
        }
        Type::BareFn(value) => {
            fields.insert(
                "tokens".into(),
                Value::String(value.to_token_stream().to_string()),
            );
            "BareFn"
        }
        Type::Group(value) => {
            fields.insert(
                "elem".into(),
                type_ast(file, &child(pointer, "elem"), &value.elem, source_map)?,
            );
            "Group"
        }
        Type::ImplTrait(value) => {
            fields.insert(
                "bounds".into(),
                token_value(file, &child(pointer, "bounds"), &value.bounds, source_map)?,
            );
            "ImplTrait"
        }
        Type::Infer(_) => "Infer",
        Type::Macro(value) => {
            fields.insert(
                "mac".into(),
                macro_ast(file, &child(pointer, "mac"), &value.mac, source_map)?,
            );
            "Macro"
        }
        Type::Never(_) => "Never",
        Type::Paren(value) => {
            fields.insert(
                "elem".into(),
                type_ast(file, &child(pointer, "elem"), &value.elem, source_map)?,
            );
            "Paren"
        }
        Type::Path(value) => {
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
        Type::Ptr(value) => {
            fields.insert(
                "const_token".into(),
                token_value(
                    file,
                    &child(pointer, "const_token"),
                    &value.const_token,
                    source_map,
                )?,
            );
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
                "elem".into(),
                type_ast(file, &child(pointer, "elem"), &value.elem, source_map)?,
            );
            "Ptr"
        }
        Type::Reference(value) => {
            fields.insert(
                "lifetime".into(),
                token_value(
                    file,
                    &child(pointer, "lifetime"),
                    &value.lifetime,
                    source_map,
                )?,
            );
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
                "elem".into(),
                type_ast(file, &child(pointer, "elem"), &value.elem, source_map)?,
            );
            "Reference"
        }
        Type::Slice(value) => {
            fields.insert(
                "elem".into(),
                type_ast(file, &child(pointer, "elem"), &value.elem, source_map)?,
            );
            "Slice"
        }
        Type::TraitObject(value) => {
            fields.insert(
                "bounds".into(),
                token_value(file, &child(pointer, "bounds"), &value.bounds, source_map)?,
            );
            "TraitObject"
        }
        Type::Tuple(value) => {
            let mut elems = Vec::with_capacity(value.elems.len());
            for (index, elem) in value.elems.iter().enumerate() {
                elems.push(type_ast(
                    file,
                    &pointer_child(&child(pointer, "elems"), &index.to_string()),
                    elem,
                    source_map,
                )?);
            }
            fields.insert("elems".into(), Value::Array(elems));
            "Tuple"
        }
        Type::Verbatim(tokens) => {
            fields.insert("tokens".into(), Value::String(tokens.to_string()));
            "Verbatim"
        }
        _ => {
            fields.insert(
                "tokens".into(),
                Value::String(value.to_token_stream().to_string()),
            );
            "TypeOther"
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

fn pat_ast(
    file: &SourceFile,
    pointer: &str,
    value: &Pat,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    let kind = match value {
        Pat::Ident(value) => {
            fields.insert(
                "ident".into(),
                ident(file, &child(pointer, "ident"), &value.ident, source_map)?,
            );
            fields.insert(
                "by_ref".into(),
                token_value(file, &child(pointer, "by_ref"), &value.by_ref, source_map)?,
            );
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
                "subpat".into(),
                value.subpat.as_ref().map_or(Ok(Value::Null), |(_, pat)| {
                    pat_ast(file, &child(pointer, "subpat"), pat, source_map)
                })?,
            );
            "Ident"
        }
        Pat::Lit(value) => {
            fields.insert(
                "lit".into(),
                Value::String(value.lit.to_token_stream().to_string()),
            );
            "Lit"
        }
        Pat::Or(value) => {
            let mut cases = Vec::with_capacity(value.cases.len());
            for (index, pat) in value.cases.iter().enumerate() {
                cases.push(pat_ast(
                    file,
                    &pointer_child(&child(pointer, "cases"), &index.to_string()),
                    pat,
                    source_map,
                )?);
            }
            fields.insert("cases".into(), Value::Array(cases));
            "Or"
        }
        Pat::Paren(value) => {
            fields.insert(
                "pat".into(),
                pat_ast(file, &child(pointer, "pat"), &value.pat, source_map)?,
            );
            "Paren"
        }
        Pat::Path(value) => {
            fields.insert(
                "path".into(),
                path_ast(file, &child(pointer, "path"), &value.path, source_map)?,
            );
            "Path"
        }
        Pat::Reference(value) => {
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
                "pat".into(),
                pat_ast(file, &child(pointer, "pat"), &value.pat, source_map)?,
            );
            "Reference"
        }
        Pat::Rest(_) => "Rest",
        Pat::Slice(value) => {
            let mut elems = Vec::with_capacity(value.elems.len());
            for (index, pat) in value.elems.iter().enumerate() {
                elems.push(pat_ast(
                    file,
                    &pointer_child(&child(pointer, "elems"), &index.to_string()),
                    pat,
                    source_map,
                )?);
            }
            fields.insert("elems".into(), Value::Array(elems));
            "Slice"
        }
        Pat::Struct(value) => {
            fields.insert(
                "path".into(),
                path_ast(file, &child(pointer, "path"), &value.path, source_map)?,
            );
            fields.insert(
                "fields_tokens".into(),
                Value::String(value.fields.to_token_stream().to_string()),
            );
            "Struct"
        }
        Pat::Tuple(value) => {
            let mut elems = Vec::with_capacity(value.elems.len());
            for (index, pat) in value.elems.iter().enumerate() {
                elems.push(pat_ast(
                    file,
                    &pointer_child(&child(pointer, "elems"), &index.to_string()),
                    pat,
                    source_map,
                )?);
            }
            fields.insert("elems".into(), Value::Array(elems));
            "Tuple"
        }
        Pat::TupleStruct(value) => {
            fields.insert(
                "path".into(),
                path_ast(file, &child(pointer, "path"), &value.path, source_map)?,
            );
            let mut elems = Vec::with_capacity(value.elems.len());
            for (index, pat) in value.elems.iter().enumerate() {
                elems.push(pat_ast(
                    file,
                    &pointer_child(&child(pointer, "elems"), &index.to_string()),
                    pat,
                    source_map,
                )?);
            }
            fields.insert("elems".into(), Value::Array(elems));
            "TupleStruct"
        }
        Pat::Type(value) => {
            fields.insert(
                "pat".into(),
                pat_ast(file, &child(pointer, "pat"), &value.pat, source_map)?,
            );
            fields.insert(
                "ty".into(),
                type_ast(file, &child(pointer, "ty"), &value.ty, source_map)?,
            );
            "Type"
        }
        Pat::Wild(_) => "Wild",
        Pat::Macro(value) => {
            fields.insert(
                "mac".into(),
                macro_ast(file, &child(pointer, "mac"), &value.mac, source_map)?,
            );
            "Macro"
        }
        Pat::Verbatim(tokens) => {
            fields.insert("tokens".into(), Value::String(tokens.to_string()));
            "Verbatim"
        }
        _ => {
            fields.insert(
                "tokens".into(),
                Value::String(value.to_token_stream().to_string()),
            );
            "PatOther"
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

fn path_ast(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Path,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut segments = Vec::with_capacity(value.segments.len());
    for (index, segment) in value.segments.iter().enumerate() {
        let at = pointer_child(&child(pointer, "segments"), &index.to_string());
        let mut fields = Map::new();
        fields.insert(
            "ident".into(),
            ident(file, &child(&at, "ident"), &segment.ident, source_map)?,
        );
        fields.insert(
            "arguments".into(),
            token_value(
                file,
                &child(&at, "arguments"),
                &segment.arguments,
                source_map,
            )?,
        );
        segments.push(node(
            file,
            &at,
            "PathSegment",
            fields,
            segment.span(),
            false,
            source_map,
        ));
    }
    Ok(node(
        file,
        pointer,
        "Path",
        Map::from_iter([("segments".to_owned(), Value::Array(segments))]),
        value.span(),
        true,
        source_map,
    ))
}

fn ident(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Ident,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let span = value.span();
    let range = span_range(file, span);
    let mut fields = Map::new();
    fields.insert("name".into(), Value::String(value.to_string()));
    let value = node(file, pointer, "Ident", fields, span, false, source_map);
    if let Some(range) = range {
        source_map.insert(
            child(pointer, "name"),
            Mapping {
                range: Some(range),
                text_pointer: None,
                text_map: None,
            },
        );
    }
    Ok(value)
}

fn ident_ast(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Ident,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    ident(file, pointer, value, source_map)
}

fn visibility(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Visibility,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    match value {
        syn::Visibility::Public(_) => {}
        syn::Visibility::Restricted(value) => {
            fields.insert(
                "path".into(),
                path_ast(file, &child(pointer, "path"), &value.path, source_map)?,
            );
            fields.insert(
                "in_token".into(),
                Value::String(value.in_token.to_token_stream().to_string()),
            );
        }
        syn::Visibility::Inherited => {}
    }
    Ok(node(
        file,
        pointer,
        match value {
            syn::Visibility::Public(_) => "Public",
            syn::Visibility::Restricted(_) => "Restricted",
            syn::Visibility::Inherited => "Inherited",
        },
        fields,
        value.span(),
        false,
        source_map,
    ))
}

fn attributes(
    file: &SourceFile,
    pointer: &str,
    values: &[syn::Attribute],
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut output = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let at = pointer_child(pointer, &index.to_string());
        let mut fields = Map::new();
        fields.insert(
            "style".into(),
            Value::String(match value.style {
                syn::AttrStyle::Outer => "outer".to_owned(),
                syn::AttrStyle::Inner(_) => "inner".to_owned(),
            }),
        );
        fields.insert(
            "meta".into(),
            Value::String(value.meta.to_token_stream().to_string()),
        );
        fields.insert(
            "path".into(),
            path_ast(file, &child(&at, "path"), value.path(), source_map)?,
        );
        output.push(node(
            file,
            &at,
            "Attribute",
            fields,
            value.span(),
            false,
            source_map,
        ));
    }
    Ok(Value::Array(output))
}

fn macro_ast(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Macro,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut fields = Map::new();
    fields.insert(
        "path".into(),
        path_ast(file, &child(pointer, "path"), &value.path, source_map)?,
    );
    fields.insert("tokens".into(), Value::String(value.tokens.to_string()));
    Ok(node(
        file,
        pointer,
        "Macro",
        fields,
        value.span(),
        true,
        source_map,
    ))
}

fn token_value<T: ToTokens + Spanned>(
    file: &SourceFile,
    pointer: &str,
    value: &T,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let range = span_range(file, value.span());
    let output = Value::String(value.to_token_stream().to_string());
    if let Some(range) = range {
        source_map.insert(
            pointer.to_owned(),
            Mapping {
                range: Some(range),
                text_pointer: None,
                text_map: None,
            },
        );
    }
    Ok(output)
}

fn array_tokens<T: ToTokens + Spanned>(
    file: &SourceFile,
    pointer: &str,
    values: &[T],
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let mut output = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        output.push(token_value(
            file,
            &pointer_child(pointer, &index.to_string()),
            value,
            source_map,
        )?);
    }
    Ok(Value::Array(output))
}

fn impl_item(
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

fn trait_item(
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

fn block_ast(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Block,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    block(file, pointer, value, source_map)
}

fn child(parent: &str, key: &str) -> String {
    pointer_child(parent, key)
}

fn node(
    file: &SourceFile,
    pointer: &str,
    kind: &str,
    mut fields: Map<String, Value>,
    span: proc_macro2::Span,
    source_bearing: bool,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Value {
    let mut text_pointer = None;
    let range = span_range(file, span);
    if let Some([start, end]) = range {
        if source_bearing {
            let pointer_to_source = child(pointer, "source");
            let from = start as usize;
            let to = end as usize;
            if let Some(text) = file.text.get(from..to) {
                fields.insert("source".to_owned(), Value::String(text.to_owned()));
                source_map.insert(
                    pointer_to_source.clone(),
                    Mapping {
                        range: Some([start, end]),
                        text_pointer: None,
                        text_map: (!text.is_empty()).then(|| {
                            vec![TextMapSegment {
                                text_range: [0, text.len() as u32],
                                source_range: [start, end],
                            }]
                        }),
                    },
                );
                text_pointer = Some(pointer_to_source);
            }
        }
        source_map.insert(
            pointer.to_owned(),
            Mapping {
                range: Some([start, end]),
                text_pointer,
                text_map: None,
            },
        );
    }
    fields.insert("kind".to_owned(), Value::String(kind.to_owned()));
    Value::Object(fields)
}

fn span_range(file: &SourceFile, span: proc_macro2::Span) -> Option<[u32; 2]> {
    let start = span.start();
    let end = span.end();
    let start = file.offset(start.line, start.column)?;
    let end = file.offset(end.line, end.column)?;
    if start > end
        || end > file.text.len()
        || !file.text.is_char_boundary(start)
        || !file.text.is_char_boundary(end)
    {
        return None;
    }
    Some([u32::try_from(start).ok()?, u32::try_from(end).ok()?])
}

fn offset(value: usize) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "source file exceeds u32 byte offsets".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_file;

    #[test]
    fn rust_ast_keeps_nested_items_and_pointer_ranges() {
        let text = "pub fn greet(person: &str) { let value = person.trim(); println!(\"hello {value}\"); }\nstruct Person { name: String }\n";
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        let file = SourceFile {
            rel: "src/lib.rs".to_owned(),
            text: text.to_owned(),
            ast: parse_file(text).unwrap(),
            generated: false,
            line_starts,
        };
        let serialized = from_file(&file).unwrap();
        let function = serialized.value_at("/items/0").unwrap();
        assert_eq!(function["kind"], "ItemFn");
        assert_eq!(function.pointer("/sig/ident/name").unwrap(), "greet");
        assert_eq!(
            function.pointer("/sig/inputs/0/pat/ident/name").unwrap(),
            "person"
        );
        assert_eq!(function.pointer("/block/stmts/0/kind").unwrap(), "Local");
        let source_map = serialized.mapping.get("/items/0").unwrap();
        assert_eq!(source_map.text_pointer.as_deref(), Some("/items/0/source"));
        assert!(serialized.mapping.contains_key("/items/0/block/stmts/0"));
        assert!(
            serialized.validate(text).is_ok(),
            "{:?}",
            serialized.validate(text)
        );
    }

    #[test]
    fn identical_nested_expressions_keep_distinct_pointers_and_ranges() {
        let text = "fn pair() { consume(\"same\", \"same\"); }\n";
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        let file = SourceFile {
            rel: "src/lib.rs".to_owned(),
            text: text.to_owned(),
            ast: parse_file(text).unwrap(),
            generated: false,
            line_starts,
        };
        let ast = from_file(&file).unwrap();
        let first = ast
            .mapping
            .get("/items/0/block/stmts/0/expr/args/0")
            .unwrap()
            .range
            .unwrap();
        let second = ast
            .mapping
            .get("/items/0/block/stmts/0/expr/args/1")
            .unwrap()
            .range
            .unwrap();
        assert!(first[0] < second[0], "{first:?} {second:?}");
        assert_eq!(
            ast.value_at("/items/0/block/stmts/0/expr/args/0").unwrap()["kind"],
            "Lit"
        );
    }
}
