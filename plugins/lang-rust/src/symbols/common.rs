use super::*;

pub(super) fn attributes(
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

pub(super) fn macro_ast(
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

pub(super) fn token_value<T: ToTokens + Spanned>(
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

pub(super) fn array_tokens<T: ToTokens + Spanned>(
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

pub(super) fn child(parent: &str, key: &str) -> String {
    pointer_child(parent, key)
}

pub(super) fn node(
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

pub(super) fn span_range(file: &SourceFile, span: proc_macro2::Span) -> Option<[u32; 2]> {
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

pub(super) fn offset(value: usize) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "source file exceeds u32 byte offsets".to_owned())
}
