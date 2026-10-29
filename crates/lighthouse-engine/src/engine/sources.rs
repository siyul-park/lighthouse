use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use globset::{Glob, GlobSetBuilder};
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

struct CompiledRule<'a> {
    spec: &'a SourceRuleSpec,
    select: Selector,
    language_from: Option<Selector>,
}

impl<'a> CompiledRule<'a> {
    fn new(spec: &'a SourceRuleSpec) -> Self {
        Self {
            spec,
            select: Selector::parse(&spec.select)
                .expect("source selectors are validated by Config"),
            language_from: spec.language.from.as_deref().map(|selector| {
                Selector::parse(selector).expect("source selectors are validated by Config")
            }),
        }
    }
}

/// The id of the provider a language name stands for, when one that the run
/// has enabled declares it.
pub(super) type Named<'a> = &'a dyn Fn(&str) -> Option<String>;

pub(super) fn select(
    rules: &[SourceRuleSpec],
    sources: Vec<Source>,
    inputs: &[Input],
    named: Named<'_>,
    run: &mut Run,
) -> Vec<EmbeddedFile> {
    let files: BTreeMap<_, _> = inputs
        .iter()
        .map(|input| (input.file.path.as_path(), input))
        .collect();
    let rules: Vec<_> = rules.iter().map(CompiledRule::new).collect();
    let mut matcher = GlobSetBuilder::new();
    let mut owners = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        for pattern in &rule.spec.files {
            matcher.add(Glob::new(pattern).expect("source globs are validated by Config"));
            owners.push(index);
        }
    }
    let matcher = matcher
        .build()
        .expect("source globs are validated by Config");
    let asts = accepted(sources, &files, run);
    let mut output = Vec::new();
    for (path, input) in files {
        let matching: BTreeSet<_> = matcher
            .matches(path)
            .into_iter()
            .map(|pattern| owners[pattern])
            .collect();
        if matching.is_empty() {
            continue;
        }

        let Some(source_ast) = asts.get(path) else {
            if !run
                .incomplete
                .iter()
                .any(|gap| gap.path.as_deref() == Some(path))
            {
                run.incomplete.push(Incomplete {
                    path: Some(path.to_path_buf()),
                    reason: format!(
                        "source rule(s) {} require a source AST, but the host provider returned none",
                        matching
                            .iter()
                            .map(|index| rules[*index].spec.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
            continue;
        };

        let mut selected = Vec::new();
        for index in matching {
            let rule = &rules[index];
            match select_rule(rule, source_ast, named) {
                Ok(regions) => selected.extend(regions),
                Err(reason) => run.incomplete.push(Incomplete {
                    path: Some(path.to_path_buf()),
                    reason: format!("source rule {}: {reason}", rule.spec.id),
                }),
            }
        }
        if !selected.is_empty() {
            output.push(EmbeddedFile {
                file: input.file.clone(),
                text: input.text.clone(),
                sources: selected,
            });
        }
    }
    output
}

/// The source ASTs the host providers returned that can be used: for a
/// requested file, once, and valid against its text.
fn accepted(
    sources: Vec<Source>,
    files: &BTreeMap<&Path, &Input>,
    run: &mut Run,
) -> BTreeMap<PathBuf, Source> {
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
    asts
}

fn select_rule(
    rule: &CompiledRule<'_>,
    source_ast: &Source,
    named: Named<'_>,
) -> Result<Vec<EmbeddedSource>, String> {
    let spec = rule.spec;
    let mut output = Vec::new();
    for candidate in evaluate_selector(source_ast, "", &rule.select)? {
        let provider = if let Some(provider) = &spec.language.provider {
            provider.clone()
        } else {
            let selector = rule
                .language_from
                .as_ref()
                .ok_or_else(|| "language requires exactly one of provider or from".to_owned())?;
            let values = evaluate_selector(source_ast, &candidate.pointer, selector)?;
            // A candidate with no name, or one no enabled language answers
            // to, is not code: it is text, and nothing is said of it.
            let [value] = values.as_slice() else {
                continue;
            };
            let Some(provider) = scalar(value.value)?.as_str().and_then(named) else {
                continue;
            };
            provider
        };

        let context = resolve_context(spec, source_ast, &candidate)?;
        let source = resolve_source(spec, source_ast, &candidate)?;
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
            spec.id.len(),
            spec.id,
            path.len(),
            path,
            candidate.pointer.len(),
            candidate.pointer
        );
        output.push(EmbeddedSource {
            id,
            rule_id: spec.id.clone(),
            node_pointer: candidate.pointer,
            source_pointer: source.pointer,
            provider,
            syntax: spec.language.syntax.clone(),
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
    if let Some(identity) = configured.get("identity") {
        context.insert(
            "identity".to_owned(),
            resolve_identity(source_ast, candidate, identity)?,
        );
    }
    if let Some(variable) = configured.get("variable") {
        context.insert(
            "variable".to_owned(),
            resolve_variable(source_ast, candidate, variable)?,
        );
    }
    for (key, value) in configured {
        if matches!(key.as_str(), "identity" | "variable") {
            continue;
        }
        context.insert(
            key.clone(),
            resolve_projection(source_ast, candidate, key, value)?,
        );
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
    evaluate_selector(source_ast, base_pointer, &Selector::parse(query)?)
}

fn evaluate_selector<'a>(
    source_ast: &'a Source,
    base_pointer: &str,
    selector: &Selector,
) -> Result<Vec<Selected<'a>>, String> {
    let base = pointer_value(&source_ast.ast, base_pointer)
        .ok_or_else(|| format!("selector base pointer {base_pointer} does not resolve"))?;
    let mut current = vec![Selected {
        value: base,
        pointer: base_pointer.to_owned(),
    }];

    for part in selector.parts() {
        let mut next = Vec::new();
        for parent in current {
            match (part, parent.value) {
                (SelectorPart::Filter(filter), _) => {
                    next.extend(filter_matches(source_ast, &parent, filter)?.then_some(parent));
                }
                (SelectorPart::Descend, _) => descend(source_ast, &parent, &mut next),
                (SelectorPart::Key(key), Value::Object(object)) => {
                    next.extend(object.get(key).map(|value| Selected {
                        value,
                        pointer: pointer_child(&parent.pointer, key),
                    }));
                }
                (SelectorPart::Index(index), Value::Array(array)) => {
                    next.extend(array.get(*index).map(|value| Selected {
                        value,
                        pointer: pointer_child(&parent.pointer, &index.to_string()),
                    }));
                }
                (SelectorPart::Keys, Value::Object(object)) => {
                    next.extend(object_children(source_ast, &parent.pointer, object));
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

/// The values of `object` below `pointer`, in the order of their source.
fn object_children<'a>(
    source_ast: &Source,
    pointer: &str,
    object: &'a Map<String, Value>,
) -> Vec<Selected<'a>> {
    let mut children: Vec<_> = object
        .iter()
        .map(|(key, value)| Selected {
            value,
            pointer: pointer_child(pointer, key),
        })
        .collect();
    children.sort_by_key(|item| {
        source_ast
            .mapping
            .get(&item.pointer)
            .and_then(|entry| entry.range)
            .map_or(u32::MAX, |range| range[0])
    });
    children
}

/// Adds `parent` and every value below it, parents first.
fn descend<'a>(source_ast: &Source, parent: &Selected<'a>, found: &mut Vec<Selected<'a>>) {
    found.push(parent.clone());
    let children: Vec<Selected<'a>> = match parent.value {
        Value::Object(object) => object_children(source_ast, &parent.pointer, object),
        Value::Array(array) => array
            .iter()
            .enumerate()
            .map(|(index, value)| Selected {
                value,
                pointer: pointer_child(&parent.pointer, &index.to_string()),
            })
            .collect(),
        _ => return,
    };
    for child in &children {
        descend(source_ast, child, found);
    }
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
    let values = evaluate_selector(source_ast, &candidate.pointer, &comparison.path)?;
    let matched = values
        .iter()
        .map(|item| scalar(item.value))
        .collect::<Result<Vec<_>, _>>()?;
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
mod tests;
