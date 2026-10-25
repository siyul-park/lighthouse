use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use globset::Glob;
use lighthouse_model::Incomplete;
use lighthouse_plugin::{EmbeddedFile, EmbeddedSource};
use lighthouse_protocol::Source;
use lighthouse_protocol::{
    Selector, SelectorComparison, SelectorFilter, SelectorPart, pointer_child, pointer_tokens,
    pointer_value,
};
use lighthouse_spec::SourceRuleSpec;
use serde_json::{Map, Value};

use super::{Input, Run};

#[derive(Clone)]
struct Selected<'a> {
    value: &'a Value,
    pointer: String,
}

pub(super) fn select(
    rules: &[SourceRuleSpec],
    sources: Vec<Source>,
    inputs: &[Input],
    run: &mut Run,
) -> Vec<EmbeddedFile> {
    let files: BTreeMap<_, _> = inputs
        .iter()
        .map(|input| (input.file.path.as_path(), input))
        .collect();
    let mut asts = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();

    for source_ast in sources {
        let path = PathBuf::from(&source_ast.path);
        let Some(input) = files.get(path.as_path()) else {
            run.notices.insert(format!(
                "{}: ignored source AST for an unrequested file",
                path.display()
            ));
            continue;
        };
        if !seen.insert(path.clone()) {
            asts.remove(&path);
            if duplicates.insert(path.clone()) {
                run.incomplete.push(Incomplete {
                    path: Some(path.clone()),
                    reason: "provider returned duplicate source AST records for one physical file"
                        .to_owned(),
                });
            }
            continue;
        }
        if let Err(reason) = source_ast.validate(&input.text) {
            run.incomplete.push(Incomplete {
                path: Some(path),
                reason: format!("invalid source AST: {reason}"),
            });
            continue;
        }
        asts.insert(path, source_ast);
    }

    for input in inputs {
        let path = &input.file.path;
        let matching: Vec<&str> = rules
            .iter()
            .filter(|rule| matches_file(rule, path))
            .map(|rule| rule.id.as_str())
            .collect();
        if matching.is_empty() || asts.contains_key(path) {
            continue;
        }
        if run
            .incomplete
            .iter()
            .any(|gap| gap.path.as_deref() == Some(path.as_path()))
        {
            continue;
        }
        run.incomplete.push(Incomplete {
            path: Some(path.clone()),
            reason: format!(
                "source rule(s) {} require a source AST, but the host provider returned none",
                matching.join(", ")
            ),
        });
    }

    let mut output = Vec::new();
    for (path, source_ast) in asts {
        let Some(input) = files.get(path.as_path()) else {
            continue;
        };
        let matching = rules.iter().filter(|rule| matches_file(rule, &path));
        let mut sources = Vec::new();
        for rule in matching {
            match select_rule(rule, &source_ast) {
                Ok(regions) => sources.extend(regions),
                Err(reason) => run.incomplete.push(Incomplete {
                    path: Some(path.clone()),
                    reason: format!("source rule {}: {reason}", rule.id),
                }),
            }
        }
        if !sources.is_empty() {
            output.push(EmbeddedFile {
                file: input.file.clone(),
                text: input.text.clone(),
                sources,
            });
        }
    }
    output
}

fn matches_file(rule: &SourceRuleSpec, path: &Path) -> bool {
    rule.files
        .iter()
        .any(|pattern| Glob::new(pattern).is_ok_and(|glob| glob.compile_matcher().is_match(path)))
}

fn select_rule(rule: &SourceRuleSpec, source_ast: &Source) -> Result<Vec<EmbeddedSource>, String> {
    let mut output = Vec::new();
    for candidate in evaluate(source_ast, "", &rule.select)? {
        let provider = if let Some(provider) = &rule.language.provider {
            provider.clone()
        } else {
            let selector =
                rule.language.select.as_deref().ok_or_else(|| {
                    "language requires exactly one of provider or select".to_owned()
                })?;
            let values = evaluate(source_ast, &candidate.pointer, selector)?;
            if values.len() != 1 {
                return Err(format!(
                    "language.select returned {} values; expected exactly one provider key",
                    values.len()
                ));
            }
            scalar(values[0].value)?
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "language.select did not return a string provider key".to_owned())?
        };

        let context = resolve_context(rule, source_ast, &candidate)?;
        let source = resolve_source(rule, source_ast, &candidate)?;
        let source_text = source
            .value
            .as_str()
            .ok_or_else(|| format!("source pointer {} is not a string scalar", source.pointer))?;
        let source_entry = source_ast
            .mapping
            .get(&source.pointer)
            .ok_or_else(|| format!("source scalar {} has no source-map entry", source.pointer))?;
        let source_range = source_entry.range.ok_or_else(|| {
            format!(
                "source scalar {} has no original source range",
                source.pointer
            )
        })?;

        let path = &source_ast.path;
        let id = format!(
            "{}:{}:{}:{}:{}:{}",
            rule.id.len(),
            rule.id,
            path.len(),
            path,
            candidate.pointer.len(),
            candidate.pointer
        );
        output.push(EmbeddedSource {
            id,
            rule_id: rule.id.clone(),
            node_pointer: candidate.pointer,
            source_pointer: source.pointer,
            provider,
            syntax: rule.language.syntax.clone(),
            context,
            text: source_text.to_owned(),
            source_range,
            text_map: source_entry.text_map.clone(),
        });
    }
    Ok(output)
}

fn resolve_source<'a>(
    rule: &SourceRuleSpec,
    source_ast: &'a Source,
    candidate: &Selected<'a>,
) -> Result<Selected<'a>, String> {
    if let Some(variable) = rule
        .language
        .context
        .as_ref()
        .and_then(|value| value.get("variable"))
    {
        let selector = variable
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| "language.context.variable.value must be a selector".to_owned())?;
        let values = evaluate(source_ast, &candidate.pointer, selector)?;
        if values.len() != 1 {
            return Err(format!(
                "language.context.variable.value returned {} values; expected exactly one source",
                values.len()
            ));
        }
        if !values[0].value.is_string() {
            return Err(
                "language.context.variable.value did not return a string scalar".to_owned(),
            );
        }
        return Ok(values[0].clone());
    }

    if candidate.value.is_string() {
        return Ok(candidate.clone());
    }

    let text_pointer = source_ast
        .mapping
        .get(&candidate.pointer)
        .and_then(|entry| entry.text_pointer.as_deref())
        .ok_or_else(|| {
            format!(
                "selected node {} is not a string scalar and has no explicit text_pointer",
                candidate.pointer
            )
        })?;
    let value = pointer_value(&source_ast.ast, text_pointer)
        .ok_or_else(|| format!("text_pointer {text_pointer} does not resolve in the AST"))?;
    if !value.is_string() {
        return Err(format!(
            "text_pointer {text_pointer} does not identify a string scalar"
        ));
    }
    Ok(Selected {
        value,
        pointer: text_pointer.to_owned(),
    })
}

fn resolve_context(
    rule: &SourceRuleSpec,
    source_ast: &Source,
    candidate: &Selected<'_>,
) -> Result<Value, String> {
    let Some(configured) = rule.language.context.as_ref().and_then(Value::as_object) else {
        return Ok(Value::Object(Map::new()));
    };
    let mut context = Map::new();
    for (key, value) in configured {
        let resolved = match key.as_str() {
            "identity" => resolve_identity(source_ast, candidate, value)?,
            "variable" => resolve_variable(source_ast, candidate, value)?,
            _ => resolve_projection(source_ast, candidate, key, value)?,
        };
        context.insert(key.clone(), resolved);
    }
    Ok(Value::Object(context))
}

fn resolve_identity(
    source_ast: &Source,
    candidate: &Selected<'_>,
    value: &Value,
) -> Result<Value, String> {
    let identity = value.as_object().ok_or_else(|| {
        "language.context.identity must be an object of root-relative selectors".to_owned()
    })?;
    let mut resolved = Map::new();
    for (name, selector) in identity {
        let selector = selector
            .as_str()
            .ok_or_else(|| format!("language.context.identity.{name} must be a selector"))?;
        let values = identity_values(source_ast, candidate, selector)?;
        if values.len() != 1 {
            return Err(format!(
                "identity selector {selector} returned {}; expected one value for the selected logical root",
                values.len()
            ));
        }
        resolved.insert(name.clone(), scalar(values[0].value)?);
    }
    Ok(Value::Object(resolved))
}

fn resolve_variable(
    source_ast: &Source,
    candidate: &Selected<'_>,
    value: &Value,
) -> Result<Value, String> {
    let variable = value
        .as_object()
        .ok_or_else(|| "language.context.variable must be an object".to_owned())?;
    let mut resolved = Map::new();
    for (field, setting) in variable {
        if field == "value" {
            continue;
        }
        match field.as_str() {
            "name" => {
                let selector = setting.as_str().ok_or_else(|| {
                    "language.context.variable.name must be a selector".to_owned()
                })?;
                let values = evaluate(source_ast, &candidate.pointer, selector)?;
                if values.len() != 1 {
                    return Err(format!(
                        "variable.name selector {selector} returned {}; expected one value",
                        values.len()
                    ));
                }
                let value = scalar(values[0].value)?;
                let range = source_range(source_ast, &values[0].pointer)?;
                resolved.insert(field.clone(), value);
                resolved.insert("nameStart".to_owned(), Value::from(range[0]));
                resolved.insert("nameEnd".to_owned(), Value::from(range[1]));
            }
            "params" => {
                let selector = setting.as_str().ok_or_else(|| {
                    "language.context.variable.params must be a selector".to_owned()
                })?;
                let selected = evaluate(source_ast, &candidate.pointer, selector)?;
                let mut values = Vec::with_capacity(selected.len());
                let mut spans = Vec::with_capacity(selected.len());
                for item in selected {
                    let value = scalar(item.value)?;
                    let range = source_range(source_ast, &item.pointer)?;
                    spans.push(serde_json::json!({
                        "name": value.as_str().unwrap_or(""),
                        "start": range[0],
                        "end": range[1]
                    }));
                    values.push(value);
                }
                resolved.insert(field.clone(), Value::Array(values));
                resolved.insert("paramsSpans".to_owned(), Value::Array(spans));
            }
            _ => {
                resolved.insert(field.clone(), setting.clone());
            }
        }
    }
    let extent = source_range(source_ast, &candidate.pointer)?;
    resolved.insert("extentStart".to_owned(), Value::from(extent[0]));
    resolved.insert("extentEnd".to_owned(), Value::from(extent[1]));
    Ok(Value::Object(resolved))
}

fn resolve_projection(
    source_ast: &Source,
    candidate: &Selected<'_>,
    key: &str,
    value: &Value,
) -> Result<Value, String> {
    let Some(selector) = value.as_object().and_then(|object| object.get("select")) else {
        return Ok(value.clone());
    };
    let selector = selector
        .as_str()
        .ok_or_else(|| format!("language.context.{key}.select must be a selector"))?;
    let values = evaluate(source_ast, &candidate.pointer, selector)?;
    if values.is_empty() {
        return Err(format!("context selector {selector} returned no values"));
    }
    let resolved = values
        .into_iter()
        .map(|item| scalar(item.value))
        .collect::<Result<Vec<_>, _>>()?;
    if resolved.len() == 1 {
        Ok(resolved.into_iter().next().expect("one selected value"))
    } else {
        Ok(Value::Array(resolved))
    }
}

fn identity_values<'a>(
    source: &'a Source,
    candidate: &Selected<'a>,
    selector: &str,
) -> Result<Vec<Selected<'a>>, String> {
    let values = evaluate(source, "", selector)?;
    let candidate_tokens = pointer_tokens(&candidate.pointer)
        .ok_or_else(|| format!("invalid selected JSON Pointer {}", candidate.pointer))?;
    let mut scoped = Vec::new();
    let mut longest_prefix = None;
    for value in values {
        let tokens = pointer_tokens(&value.pointer)
            .ok_or_else(|| format!("invalid identity JSON Pointer {}", value.pointer))?;
        let prefix_len = candidate_tokens
            .iter()
            .zip(&tokens)
            .take_while(|(candidate, value)| candidate == value)
            .count();
        match longest_prefix {
            Some(length) if prefix_len < length => continue,
            Some(length) if prefix_len > length => {
                scoped.clear();
                longest_prefix = Some(prefix_len);
            }
            None => longest_prefix = Some(prefix_len),
            _ => {}
        }
        scoped.push(value);
    }
    Ok(scoped)
}

fn source_range(source_ast: &Source, pointer: &str) -> Result<[u32; 2], String> {
    source_ast
        .mapping
        .get(pointer)
        .and_then(|entry| entry.range)
        .ok_or_else(|| format!("selected AST value {pointer} has no original source range"))
}

fn evaluate<'a>(
    source_ast: &'a Source,
    base_pointer: &str,
    query: &str,
) -> Result<Vec<Selected<'a>>, String> {
    let selector = Selector::parse(query)?;
    let base = pointer_value(&source_ast.ast, base_pointer)
        .ok_or_else(|| format!("selector base pointer {base_pointer} does not resolve"))?;
    let mut current = vec![Selected {
        value: base,
        pointer: base_pointer.to_owned(),
    }];

    for part in selector.parts() {
        if let SelectorPart::Filter(filter) = part {
            let mut next = Vec::new();
            for candidate in current {
                if filter_matches(source_ast, &candidate, filter)? {
                    next.push(candidate);
                }
            }
            current = next;
            continue;
        }
        let mut next = Vec::new();
        for parent in current {
            match (part, parent.value) {
                (SelectorPart::Key(key), Value::Object(object)) => {
                    if let Some(value) = object.get(key) {
                        let pointer = pointer_child(&parent.pointer, key);
                        next.push(Selected { value, pointer });
                    }
                }
                (SelectorPart::Index(index), Value::Array(array)) => {
                    if let Some(value) = array.get(*index) {
                        let pointer = pointer_child(&parent.pointer, &index.to_string());
                        next.push(Selected { value, pointer });
                    }
                }
                (SelectorPart::Keys, Value::Object(object)) => {
                    let mut children: Vec<_> = object
                        .iter()
                        .map(|(key, value)| Selected {
                            value,
                            pointer: pointer_child(&parent.pointer, key),
                        })
                        .collect();
                    children.sort_by_key(|item| {
                        source_ast
                            .mapping
                            .get(&item.pointer)
                            .and_then(|entry| entry.range)
                            .map(|range| range[0])
                            .unwrap_or(u32::MAX)
                    });
                    next.extend(children);
                }
                (SelectorPart::Indices, Value::Array(array)) => {
                    next.extend(array.iter().enumerate().map(|(index, value)| Selected {
                        value,
                        pointer: pointer_child(&parent.pointer, &index.to_string()),
                    }));
                }
                _ => {}
            }
        }
        current = next;
    }
    Ok(current)
}

fn filter_matches(
    source_ast: &Source,
    candidate: &Selected<'_>,
    filter: &SelectorFilter,
) -> Result<bool, String> {
    for clause in &filter.clauses {
        let mut all = true;
        for comparison in clause {
            if !compare(source_ast, candidate, comparison)? {
                all = false;
                break;
            }
        }
        if all {
            return Ok(true);
        }
    }
    Ok(false)
}

fn compare(
    source_ast: &Source,
    candidate: &Selected<'_>,
    comparison: &SelectorComparison,
) -> Result<bool, String> {
    let values = evaluate(source_ast, &candidate.pointer, &comparison.path)?;
    let matched = values
        .iter()
        .map(|item| scalar(item.value))
        .collect::<Result<Vec<_>, _>>()?;
    if matched.is_empty() {
        return Ok(false);
    }
    Ok(if comparison.equal {
        matched.iter().any(|value| value == &comparison.value)
    } else {
        matched.iter().all(|value| value != &comparison.value)
    })
}

fn scalar(value: &Value) -> Result<Value, String> {
    match value {
        Value::Object(_) | Value::Array(_) => {
            Err("selector comparison/context value must be a scalar".to_owned())
        }
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lighthouse_protocol::TextMapSegment;
    use lighthouse_spec::SourceLanguageSpec;
    use std::collections::BTreeMap;

    fn entry(range: [u32; 2]) -> lighthouse_plugin::Mapping {
        lighthouse_plugin::Mapping {
            range: Some(range),
            text_pointer: None,
            text_map: None,
        }
    }

    #[test]
    fn selectors_preserve_json_pointers_when_values_repeat() {
        let ast = Source {
            path: "doc.yaml".to_owned(),
            ast: serde_json::json!({"items":["same", "same"]}),
            mapping: BTreeMap::from([
                ("".to_owned(), entry([0, 9])),
                ("/items".to_owned(), entry([0, 9])),
                ("/items/0".to_owned(), entry([0, 4])),
                ("/items/1".to_owned(), entry([5, 9])),
            ]),
        };
        let selected = evaluate(&ast, "", "$.items[*]").unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|item| item.pointer.as_str())
                .collect::<Vec<_>>(),
            ["/items/0", "/items/1"]
        );
        assert_eq!(selected[0].value, selected[1].value);
    }

    #[test]
    fn source_projection_uses_text_pointer_without_searching_values() {
        let host = "expr: x > 1";
        let text_map = vec![TextMapSegment {
            text_range: [0, 5],
            source_range: [6, 11],
        }];
        let ast = Source {
            path: "doc.yaml".to_owned(),
            ast: serde_json::json!({"expr-node":{"kind":"Expression","source":"x > 1"}}),
            mapping: BTreeMap::from([
                ("".to_owned(), entry([0, 11])),
                (
                    "/expr-node".to_owned(),
                    lighthouse_plugin::Mapping {
                        range: Some([0, 11]),
                        text_pointer: Some("/expr-node/source".to_owned()),
                        text_map: None,
                    },
                ),
                ("/expr-node/kind".to_owned(), entry([0, 0])),
                (
                    "/expr-node/source".to_owned(),
                    lighthouse_plugin::Mapping {
                        range: Some([6, 11]),
                        text_pointer: None,
                        text_map: Some(text_map),
                    },
                ),
            ]),
        };
        assert!(ast.validate(host).is_ok());
        let rule = SourceRuleSpec {
            id: "embedded".to_owned(),
            files: vec!["**/*.yaml".to_owned()],
            select: "$.expr-node".to_owned(),
            language: SourceLanguageSpec {
                provider: Some("cel".to_owned()),
                ..SourceLanguageSpec::default()
            },
        };
        let result = select_rule(&rule, &ast).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].node_pointer, "/expr-node");
        assert_eq!(result[0].source_pointer, "/expr-node/source");
        assert_eq!(result[0].text, "x > 1");
        assert_eq!(result[0].source_range, [6, 11]);
    }

    #[test]
    fn context_selectors_use_the_documented_scopes() {
        let text = "Decision cel x > 1";
        let ast = Source {
            path: "decisions/example.yaml".to_owned(),
            ast: serde_json::json!({
                "kind":"Decision",
                "language":"cel",
                "where":"x > 1",
            }),
            mapping: BTreeMap::from([
                ("".to_owned(), entry([0, text.len() as u32])),
                ("/kind".to_owned(), entry([0, 8])),
                ("/language".to_owned(), entry([9, 12])),
                ("/where".to_owned(), entry([13, 18])),
            ]),
        };
        let rule = SourceRuleSpec {
            id: "where".to_owned(),
            files: vec!["decisions/**/*.yaml".to_owned()],
            select: "$?(@.kind == 'Decision' && @.language == 'cel').where".to_owned(),
            language: SourceLanguageSpec {
                provider: Some("cel".to_owned()),
                context: Some(serde_json::json!({
                    "identity": {"kind":"$.kind"},
                    "declared": true
                })),
                ..SourceLanguageSpec::default()
            },
        };
        let result = select_rule(&rule, &ast).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].provider, "cel");
        assert_eq!(result[0].text, "x > 1");
        assert_eq!(result[0].context["identity"]["kind"], "Decision");
    }

    #[test]
    fn root_identity_selectors_are_scoped_to_the_candidate_document() {
        let ast = Source {
            path: "multi.yaml".to_owned(),
            ast: serde_json::json!({
                "kind": "Stream",
                "documents": [
                    {"kind":"Decision","metadata":{"name":"first"},"spec":{"where":"one"}},
                    {"kind":"Decision","metadata":{"name":"second"},"spec":{"where":"two"}}
                ]
            }),
            mapping: BTreeMap::new(),
        };
        let candidate = Selected {
            value: pointer_value(&ast.ast, "/documents/1/spec/where").unwrap(),
            pointer: "/documents/1/spec/where".to_owned(),
        };

        let kinds = identity_values(&ast, &candidate, "$.documents[*].kind").unwrap();
        let names = identity_values(&ast, &candidate, "$.documents[*].metadata.name").unwrap();

        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].pointer, "/documents/1/kind");
        assert_eq!(kinds[0].value, "Decision");
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].pointer, "/documents/1/metadata/name");
        assert_eq!(names[0].value, "second");
    }

    #[test]
    fn unsupported_composite_candidates_are_errors_not_empty_success() {
        let ast = Source {
            path: "doc.yaml".to_owned(),
            ast: serde_json::json!({"expr":{"nested":"not an explicit source pointer"}}),
            mapping: BTreeMap::from([
                ("".to_owned(), entry([0, 10])),
                ("/expr".to_owned(), entry([0, 10])),
                ("/expr/nested".to_owned(), entry([2, 9])),
            ]),
        };
        let rule = SourceRuleSpec {
            id: "embedded".to_owned(),
            files: vec!["**/*.yaml".to_owned()],
            select: "$.expr".to_owned(),
            language: SourceLanguageSpec {
                provider: Some("cel".to_owned()),
                ..SourceLanguageSpec::default()
            },
        };
        assert!(
            select_rule(&rule, &ast)
                .unwrap_err()
                .contains("no explicit text_pointer")
        );
    }
}
