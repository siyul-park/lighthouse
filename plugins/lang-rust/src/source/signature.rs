use super::*;

pub(super) fn receiver_reference(
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

pub(super) fn qself(
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

pub(super) fn signature(
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
