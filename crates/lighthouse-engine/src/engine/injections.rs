use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use lighthouse_graph::{Durability, Graph, Input as Kept};
use lighthouse_model::{
    Incomplete,
    ucm::{PAYLOAD, Payload},
};
use lighthouse_plugin::{EmbeddedFile, EmbeddedRegion};
use lighthouse_protocol::{
    Selector, SelectorComparison, SelectorFilter, SelectorMember, SelectorPart, pointer_child,
    pointer_tokens, pointer_value,
};
use lighthouse_protocol::{Source, hash::Hasher};
use lighthouse_spec::InjectionSpec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Input, Run};
use crate::graph::Stored;

/// What the regions of a host unit were selected from, kept under the key of
/// its answer. Persisted: a run that finds the key as it was selects nothing.
pub(crate) static INJECTED: Kept<(String, String), Injected> = Kept::new("injected").persisted();

/// The injections of a run, and what they select of the host files.
///
/// What a host unit's source trees select is a function of the unit's answer,
/// the injections and the languages they name, so it is kept under that key
/// ([`INJECTED`]): a warm run takes the regions and neither loads nor
/// validates a source tree.
pub(super) struct Injecting<'a> {
    rules: Vec<Injection<'a>>,
    matcher: GlobSet,
    owners: Vec<usize>,
    named: Named<'a>,
    /// What the injections and the languages they name come to, hashed.
    identity: String,
    files: BTreeMap<&'a Path, &'a Input>,
    /// The hosts the units answered for, by path: their selected regions.
    chosen: BTreeMap<PathBuf, Vec<EmbeddedRegion>>,
    /// The units whose regions are not kept, with their answers.
    pending: Vec<Pending>,
}

/// A host unit to select the regions of.
struct Pending {
    at: (String, String),
    key: String,
    files: Vec<PathBuf>,
    payload: Arc<Payload>,
}

/// What the source trees of a host unit select, under the key it was made for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Injected {
    key: String,
    notices: BTreeSet<String>,
    gaps: Vec<Incomplete>,
    /// The hosts with a source tree, with the regions selected of each, as
    /// JSON: the regions hold values the store cannot decode.
    hosts: Vec<u8>,
}

/// The hosts of a unit with a source tree, with the regions selected of each.
type Hosts = Vec<(PathBuf, Vec<EmbeddedRegion>)>;

/// The id of the provider a language name stands for, when one that the run
/// has enabled declares it.
pub(super) type Named<'a> = &'a (dyn Fn(&str) -> Option<String> + Sync);

#[derive(Clone)]
struct Selected<'a> {
    value: &'a Value,
    pointer: String,
}

/// An injection with its selectors parsed.
struct Injection<'a> {
    spec: &'a InjectionSpec,
    select: Selector,
    language_from: Option<Selector>,
}

impl<'a> Injection<'a> {
    fn new(spec: &'a InjectionSpec) -> Self {
        Self {
            spec,
            select: Selector::parse(&spec.select)
                .expect("injection selectors are validated by Config"),
            language_from: spec.language_from.as_deref().map(|selector| {
                Selector::parse(selector).expect("injection selectors are validated by Config")
            }),
        }
    }
}

impl<'a> Injecting<'a> {
    /// The injections of `injections`, over `inputs`. `languages` says which
    /// languages the names they select resolve to.
    pub(super) fn new(
        injections: &'a [InjectionSpec],
        inputs: &'a [Input],
        named: Named<'a>,
        languages: &str,
    ) -> Self {
        let rules: Vec<_> = injections.iter().map(Injection::new).collect();
        let mut matcher = GlobSetBuilder::new();
        let mut owners = Vec::new();
        for (index, rule) in rules.iter().enumerate() {
            for pattern in &rule.spec.files {
                matcher.add(Glob::new(pattern).expect("injection globs are validated by Config"));
                owners.push(index);
            }
        }
        let mut hasher = Hasher::new();
        for part in [
            crate::cache::build_identity(),
            serde_json::to_string(injections).unwrap_or_default(),
            languages.to_owned(),
        ] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        Self {
            rules,
            matcher: matcher
                .build()
                .expect("injection globs are validated by Config"),
            owners,
            named,
            identity: hasher.hex(),
            files: inputs
                .iter()
                .map(|input| (input.file.path.as_path(), input))
                .collect(),
            chosen: BTreeMap::new(),
            pending: Vec::new(),
        }
    }

    /// Takes the regions of the host unit `at`, whose answer is `stored`: from
    /// the graph when it has them for this answer, else once `settle` has
    /// selected them.
    pub(super) fn unit(
        &mut self,
        graph: &Graph,
        (at, stored, files): (&(String, String), &Stored, &[&Input]),
        run: &mut Run,
    ) {
        let mut hasher = Hasher::new();
        for part in [
            stored.key.as_bytes(),
            &stored.payload[..],
            self.identity.as_bytes(),
        ] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        let key = hasher.hex();
        if let Ok(Some(kept)) = graph.input(&INJECTED, at)
            && kept.key == key
            && let Ok(hosts) = serde_json::from_slice(&kept.hosts)
        {
            self.take(&kept, hosts, run);
            return;
        }
        let Ok(Some(payload)) = graph.input(&PAYLOAD, at) else {
            return;
        };
        self.pending.push(Pending {
            at: at.clone(),
            key,
            files: files.iter().map(|input| input.file.path.clone()).collect(),
            payload,
        });
    }

    /// Selects the regions of the units that were not kept, side by side,
    /// and keeps them.
    pub(super) fn settle(&mut self, graph: &mut Graph, run: &mut Run) {
        let pending = std::mem::take(&mut self.pending);
        let made: Vec<(Injected, Hosts)> = pending
            .par_iter()
            .map(|unit| self.select_unit(unit))
            .collect();
        for (unit, (made, hosts)) in pending.into_iter().zip(made) {
            self.take(&made, hosts, run);
            if let Err(e) = graph.set_input(&INJECTED, &unit.at, made, Durability::Volatile) {
                run.notices
                    .insert(format!("the injected regions are not kept: {e}"));
            }
        }
    }

    fn take(&mut self, made: &Injected, hosts: Hosts, run: &mut Run) {
        run.notices.extend(made.notices.iter().cloned());
        run.incomplete.extend(made.gaps.iter().cloned());
        self.chosen.extend(hosts);
    }

    /// The regions of the hosts among the files of one unit.
    fn select_unit(&self, unit: &Pending) -> (Injected, Hosts) {
        let mut hosts = Hosts::new();
        let mut made = Injected {
            key: unit.key.clone(),
            ..Injected::default()
        };
        let sources: Vec<Source> = match serde_json::from_slice(&unit.payload.sources) {
            Ok(sources) => sources,
            Err(e) => {
                made.gaps.push(Incomplete {
                    path: None,
                    provider: None,
                    reason: format!("the source trees of {} cannot be read: {e}", unit.at.1),
                });
                return (made, hosts);
            }
        };
        let asts = accepted(sources, &unit.files, &self.files, &mut made);
        let asts: Vec<_> = asts.into_iter().collect();
        let outcomes: Vec<(PathBuf, Vec<EmbeddedRegion>, Vec<Incomplete>)> = asts
            .par_iter()
            .map(|(path, source_ast)| {
                let (mut selected, mut gaps) = (Vec::new(), Vec::new());
                for index in self.matching(path) {
                    let rule = &self.rules[index];
                    match select_rule(rule, source_ast, self.named) {
                        Ok(regions) => selected.extend(regions),
                        Err(reason) => gaps.push(Incomplete {
                            path: Some(path.clone()),
                            provider: None,
                            reason: format!("injection {}: {reason}", rule.spec.id),
                        }),
                    }
                }
                (path.clone(), selected, gaps)
            })
            .collect();
        for (path, regions, gaps) in outcomes {
            made.gaps.extend(gaps);
            hosts.push((path, regions));
        }
        made.hosts = serde_json::to_vec(&hosts).unwrap_or_default();
        (made, hosts)
    }

    /// The injections that take the file.
    fn matching(&self, path: &Path) -> BTreeSet<usize> {
        self.matcher
            .matches(path)
            .into_iter()
            .map(|pattern| self.owners[pattern])
            .collect()
    }

    /// The hosts with their regions, in path order. A host no unit gave a
    /// source tree for is incomplete, unless a gap says why already.
    pub(super) fn finish(self, run: &mut Run) -> Vec<EmbeddedFile> {
        let mut output = Vec::new();
        for (&path, &input) in &self.files {
            let matching = self.matching(path);
            if matching.is_empty() {
                continue;
            }
            match self.chosen.get(path) {
                Some(regions) if !regions.is_empty() => output.push(EmbeddedFile {
                    file: input.file.clone(),
                    text: input.text.clone(),
                    regions: regions.clone(),
                }),
                Some(_) => {}
                None => {
                    if run
                        .incomplete
                        .iter()
                        .any(|gap| gap.path.as_deref() == Some(path))
                    {
                        continue;
                    }
                    let ids: Vec<_> = matching
                        .iter()
                        .map(|index| self.rules[*index].spec.id.as_str())
                        .collect();
                    run.incomplete.push(Incomplete {
                        path: Some(path.to_path_buf()),
                        provider: None,
                        reason: format!(
                            "injection(s) {} require a source AST, but the host provider returned none",
                            ids.join(", ")
                        ),
                    });
                }
            }
        }
        output
    }
}

/// The source ASTs a unit returned that can be used: for a file of the unit,
/// once, and valid against its text. What is not is said in `made`.
fn accepted(
    sources: Vec<Source>,
    unit: &[PathBuf],
    files: &BTreeMap<&Path, &Input>,
    made: &mut Injected,
) -> BTreeMap<PathBuf, Source> {
    let mut asts = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();

    let checked: Vec<Option<Result<(), String>>> = sources
        .par_iter()
        .map(|source_ast| {
            let path = Path::new(&source_ast.path);
            let input = files.get(path).filter(|_| unit.iter().any(|p| p == path))?;
            Some(source_ast.validate(&input.text))
        })
        .collect();
    for (source_ast, checked) in sources.into_iter().zip(checked) {
        let path = PathBuf::from(&source_ast.path);
        let Some(checked) = checked else {
            made.notices.insert(format!(
                "{}: ignored source AST for an unrequested file",
                path.display()
            ));
            continue;
        };
        if !seen.insert(path.clone()) {
            asts.remove(&path);
            if duplicates.insert(path.clone()) {
                made.gaps.push(Incomplete {
                    path: Some(path.clone()),
                    provider: None,
                    reason: "provider returned duplicate source AST records for one physical file"
                        .to_owned(),
                });
            }
            continue;
        }
        if let Err(reason) = checked {
            made.gaps.push(Incomplete {
                path: Some(path),
                provider: None,
                reason: format!("invalid source AST: {reason}"),
            });
            continue;
        }
        asts.insert(path, source_ast);
    }
    asts
}

fn select_rule(
    rule: &Injection<'_>,
    source_ast: &Source,
    named: Named<'_>,
) -> Result<Vec<EmbeddedRegion>, String> {
    let spec = rule.spec;
    let mut output = Vec::new();
    for candidate in evaluate_selector(source_ast, "", &rule.select)? {
        let provider = if let Some(provider) = &spec.language {
            provider.clone()
        } else {
            let selector = rule.language_from.as_ref().ok_or_else(|| {
                "an injection requires exactly one of language or languageFrom".to_owned()
            })?;
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
        output.push(EmbeddedRegion {
            id,
            injection_id: spec.id.clone(),
            node_pointer: candidate.pointer,
            source_pointer: source.pointer,
            provider,
            syntax: spec.syntax.clone(),
            context,
            text: source_text.to_owned(),
            source_range,
            text_map: source_entry.text_map.clone(),
        });
    }
    Ok(output)
}

fn resolve_source<'a>(
    rule: &InjectionSpec,
    source_ast: &'a Source,
    candidate: &Selected<'a>,
) -> Result<Selected<'a>, String> {
    if let Some(variable) = rule
        .context
        .as_ref()
        .and_then(|value| value.get("variable"))
    {
        let selector = variable
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| "context.variable.value must be a selector".to_owned())?;
        let values = evaluate(source_ast, &candidate.pointer, selector)?;
        if values.len() != 1 {
            return Err(format!(
                "context.variable.value returned {} values; expected exactly one source",
                values.len()
            ));
        }
        if !values[0].value.is_string() {
            return Err("context.variable.value did not return a string scalar".to_owned());
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
    rule: &InjectionSpec,
    source_ast: &Source,
    candidate: &Selected<'_>,
) -> Result<Value, String> {
    let Some(configured) = rule.context.as_ref().and_then(Value::as_object) else {
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
        "context.identity must be an object of root-relative selectors".to_owned()
    })?;
    let mut resolved = Map::new();
    for (name, selector) in identity {
        let selector = selector
            .as_str()
            .ok_or_else(|| format!("context.identity.{name} must be a selector"))?;
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
        .ok_or_else(|| "context.variable must be an object".to_owned())?;
    let mut resolved = Map::new();
    for (field, setting) in variable {
        if field == "value" {
            continue;
        }
        match field.as_str() {
            "name" => {
                let selector = setting
                    .as_str()
                    .ok_or_else(|| "context.variable.name must be a selector".to_owned())?;
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
                let selector = setting
                    .as_str()
                    .ok_or_else(|| "context.variable.params must be a selector".to_owned())?;
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
        .ok_or_else(|| format!("context.{key}.select must be a selector"))?;
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
                (SelectorPart::Key(key), _) => {
                    next.extend(member(&parent, &SelectorMember::Key(key.clone())));
                }
                (SelectorPart::Index(index), _) => {
                    next.extend(member(&parent, &SelectorMember::Index(*index)));
                }
                (SelectorPart::Union(members), _) => {
                    next.extend(members.iter().filter_map(|m| member(&parent, m)));
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

/// The child of `parent` a name or an index selects, if it has one.
fn member<'a>(parent: &Selected<'a>, member: &SelectorMember) -> Option<Selected<'a>> {
    let (value, step) = match (member, parent.value) {
        (SelectorMember::Key(key), Value::Object(object)) => (object.get(key)?, key.clone()),
        (SelectorMember::Index(index), Value::Array(array)) => {
            (array.get(*index)?, index.to_string())
        }
        _ => return None,
    };
    Some(Selected {
        value,
        pointer: pointer_child(&parent.pointer, &step),
    })
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
