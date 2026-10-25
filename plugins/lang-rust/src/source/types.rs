use super::*;

pub(super) fn type_ast(
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

pub(super) fn pat_ast(
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

pub(super) fn path_ast(
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

pub(super) fn ident(
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

pub(super) fn ident_ast(
    file: &SourceFile,
    pointer: &str,
    value: &syn::Ident,
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    ident(file, pointer, value, source_map)
}

pub(super) fn visibility(
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
