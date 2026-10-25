use std::collections::{BTreeMap, BTreeSet};

use cel::Env;
use lighthouse_model::{
    Incomplete,
    ucm::{Edge, EdgeKind, Fragment, Module, Node, Resolution, Symbol, SymbolId, Target},
};
use lighthouse_plugin::{
    EmbeddedFile, Error, Indexed, LanguageProvider, ProviderManifest, Source, Workspace,
};

mod analysis;
mod source;

use analysis::{Analysis, PackVariables, external_module, function_catalog};
use source::{FileDraft, MappedText, Parameter, Snippet, VariableDecl, span};

pub struct Language {
    manifest: ProviderManifest,
}

impl Language {
    pub fn new() -> Self {
        let mut manifest = ProviderManifest::new("cel", Vec::new());
        manifest.capabilities = [
            lighthouse_model::ucm::Capability::SemanticEdges,
            lighthouse_model::ucm::Capability::Extent,
            lighthouse_model::ucm::Capability::ReferenceSites,
            lighthouse_model::ucm::Capability::CompleteReferences,
            lighthouse_model::ucm::Capability::Overlays,
        ]
        .to_vec();
        Self { manifest }
    }
}

impl Default for Language {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageProvider for Language {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    fn index(&self, workspace: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        let embedded: Vec<EmbeddedFile> = files
            .iter()
            .filter(|source| !source.regions.is_empty())
            .map(|source| EmbeddedFile {
                file: source.file.clone(),
                text: source.text.to_owned(),
                sources: source.regions.to_vec(),
            })
            .collect();
        if embedded.is_empty() {
            return Ok(Indexed::default());
        }
        let mut output = Indexed::default();
        self.index_embedded_sources(workspace, &embedded, &mut output)?;
        Ok(output)
    }
}

impl Language {
    fn index_embedded_sources(
        &self,
        workspace: &Workspace,
        files: &[EmbeddedFile],
        output: &mut Indexed,
    ) -> Result<(), Error> {
        let env = Env::stdlib();
        let drafts = drafts(files, output);
        let packs = pack_variables(&drafts);
        let (fragments, external) =
            self.index_drafts(&workspace.cel_libraries, &env, &drafts, &packs, output);
        output.fragments.extend(fragments);
        let modules: BTreeSet<String> = external.values().map(external_module).collect();
        if !external.is_empty() || !modules.is_empty() {
            output.fragments.push(Fragment {
                modules: modules
                    .into_iter()
                    .map(|path| Module {
                        name: path.rsplit('/').next().map(str::to_owned),
                        path,
                        test_of: None,
                    })
                    .collect(),
                symbols: external.into_values().collect(),
                ..Fragment::default()
            });
        }
        Ok(())
    }

    fn index_drafts(
        &self,
        libraries: &[lighthouse_plugin::CelLibraryManifest],
        env: &Env,
        drafts: &[FileDraft],
        packs: &PackVariables,
        output: &mut Indexed,
    ) -> (Vec<Fragment>, BTreeMap<SymbolId, Symbol>) {
        let mut fragments = Vec::with_capacity(drafts.len());
        let mut external = function_catalog(libraries);
        for draft in drafts {
            let mut fragment = draft.fragment.clone();
            let mut analysis = Analysis::new(&draft.variables, packs, libraries, &mut external);
            declare_variables(&mut fragment, draft);
            index_snippets(env, &mut analysis, &mut fragment, draft, output);
            fragments.push(fragment);
        }
        (fragments, external)
    }
}

fn drafts(files: &[EmbeddedFile], output: &mut Indexed) -> Vec<FileDraft> {
    let mut drafts = Vec::new();
    for file in files {
        let source = Source {
            file: &file.file,
            text: &file.text,
            regions: &[],
        };
        let mut draft = FileDraft::empty(&source);
        let mut indexed = false;
        for embedded in &file.sources {
            match add_source(&mut draft, file, embedded) {
                Ok(()) => indexed = true,
                Err(reason) => output.incomplete.push(Incomplete {
                    path: Some(file.file.path.clone()),
                    reason: format!("source {}: {reason}", embedded.id),
                }),
            }
        }
        if indexed {
            drafts.push(draft);
        }
    }
    drafts
}

fn add_source(
    draft: &mut FileDraft,
    file: &EmbeddedFile,
    source: &lighthouse_plugin::EmbeddedSource,
) -> Result<(), String> {
    let template = match source.syntax.as_deref() {
        None => None,
        Some("template") => Some(template_parts(&source.text)?),
        Some(syntax) => return Err(format!("CEL does not support embedded syntax {syntax}")),
    };
    let context = &source.context;
    let variable = context.get("variable");
    let name = variable
        .and_then(|value| metadata_string_opt(value, "name"))
        .or_else(|| metadata_string_opt(context, "name"))
        .unwrap_or_else(|| source_name(source));
    let identity = context.get("identity");
    let document = metadata_string_opt(context, "document").unwrap_or_else(|| {
        identity
            .and_then(|identity| serde_json::to_string(identity).ok())
            .map(|identity| format!("{}:{}", identity, source.node_pointer))
            .unwrap_or_else(|| format!("{}:{}", file.file.path.display(), source.node_pointer))
    });
    let is_variable = variable.is_some();
    let declared = metadata_bool(context, "declared") && !is_variable;
    let public = variable.is_some_and(|value| metadata_bool(value, "public"));
    let mapped = mapped_source(source, &file.text);
    let kind = if is_variable {
        lighthouse_model::ucm::SymbolKind::Var
    } else {
        lighthouse_model::ucm::SymbolKind::Function
    };
    let id = SymbolId::new(&draft.module, &[&document, &source.id], "expression", kind);
    let params = variable_decl(draft, file, source, &document, &name, public, &id);
    if let Some(template) = template {
        for (index, (start, end)) in template.into_iter().enumerate() {
            let id = SymbolId::new(
                &draft.module,
                &[&document, &source.id, &index.to_string()],
                "expression",
                lighthouse_model::ucm::SymbolKind::Function,
            );
            draft.snippets.push(Snippet {
                doc_id: document.clone(),
                id,
                source: mapped.slice(start, end),
                name: name.clone(),
                params: params.clone(),
                declared,
            });
        }
    } else {
        draft.snippets.push(Snippet {
            doc_id: document,
            id,
            source: mapped,
            name,
            params,
            declared,
        });
    }
    Ok(())
}

fn mapped_source(source: &lighthouse_plugin::EmbeddedSource, host: &str) -> MappedText {
    let [source_start, source_end] = source.source_range.map(|n| n as usize);
    let mut starts = vec![source_start; source.text.len()];
    let mut ends = vec![source_end; source.text.len()];
    if let Some(segments) = &source.text_map {
        for segment in segments {
            let [text_start, text_end] = segment.text_range.map(|n| n as usize);
            let [mapped_start, mapped_end] = segment.source_range.map(|n| n as usize);
            let linear = text_end - text_start == mapped_end - mapped_start;
            for (offset, ch) in source.text[text_start..text_end].char_indices() {
                let from = text_start + offset;
                let to = from + ch.len_utf8();
                let source_from = if linear {
                    mapped_start + offset
                } else {
                    mapped_start
                };
                let source_to = if linear {
                    source_from + ch.len_utf8()
                } else {
                    mapped_end
                };
                for byte in from..to {
                    starts[byte] = source_from;
                    ends[byte] = source_to;
                }
            }
        }
    }
    MappedText {
        text: source.text.clone(),
        starts,
        ends,
        source_start,
        source_end,
        host: host.to_owned(),
    }
}

fn variable_decl(
    draft: &mut FileDraft,
    file: &EmbeddedFile,
    source: &lighthouse_plugin::EmbeddedSource,
    document: &str,
    name: &str,
    public: bool,
    id: &SymbolId,
) -> Vec<Parameter> {
    let Some(value) = source.context.get("variable") else {
        return Vec::new();
    };
    let position = |key: &str, fallback: u32| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(fallback as u64) as usize
    };
    let params: Vec<Parameter> = value
        .get("paramsSpans")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|param| {
            let name = param.get("name")?.as_str()?.to_owned();
            let start = param.get("start")?.as_u64()? as usize;
            let end = param.get("end")?.as_u64()? as usize;
            Some(Parameter {
                id: SymbolId::new(
                    &draft.module,
                    &[document, "variables", name.as_str()],
                    &name,
                    lighthouse_model::ucm::SymbolKind::Var,
                ),
                owner: id.clone(),
                span: span(&file.text, start, end),
                name,
            })
        })
        .collect();
    let name_start = position("nameStart", source.source_range[0]);
    let name_end = position("nameEnd", name_start as u32);
    let extent_start = position("extentStart", source.source_range[0]);
    let extent_end = position("extentEnd", source.source_range[1]);
    draft.variables.push(VariableDecl {
        doc_id: document.to_owned(),
        name: name.to_owned(),
        public,
        id: id.clone(),
        span: span(&file.text, name_start, name_end),
        extent: span(&file.text, extent_start, extent_end),
        params: params.clone(),
    });
    params
}

fn metadata_string_opt(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn source_name(source: &lighthouse_plugin::EmbeddedSource) -> String {
    source
        .node_pointer
        .rsplit('/')
        .next()
        .unwrap_or("expression")
        .replace("~1", "/")
        .replace("~0", "~")
}

fn metadata_bool(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn template_parts(text: &str) -> Result<Vec<(usize, usize)>, String> {
    let mut parts = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let open = text[cursor..].find("{{").map(|at| cursor + at);
        let close = text[cursor..].find("}}").map(|at| cursor + at);
        if let Some(close) = close
            && open.is_none_or(|open| close < open)
        {
            return Err("template has a closing delimiter without an opening delimiter".to_owned());
        }
        let Some(open) = open else {
            break;
        };
        let start = open + 2;
        let end = text[start..]
            .find("}}")
            .map(|at| start + at)
            .ok_or_else(|| "template has an unclosed expression".to_owned())?;
        let inner = &text[start..end];
        let trimmed_start = start + inner.len() - inner.trim_start().len();
        let trimmed_end = end - inner.len() + inner.trim_end().len();
        if trimmed_start >= trimmed_end {
            return Err("template contains an empty expression".to_owned());
        }
        parts.push((trimmed_start, trimmed_end));
        cursor = end + 2;
    }
    Ok(parts)
}

fn pack_variables(drafts: &[FileDraft]) -> PackVariables {
    let mut packs: PackVariables = BTreeMap::new();
    for draft in drafts {
        for variable in &draft.variables {
            if variable.public {
                packs.entry(variable.name.clone()).or_default().push((
                    variable.doc_id.clone(),
                    variable.id.clone(),
                    !variable.params.is_empty(),
                ));
            }
        }
    }
    packs
}

fn declare_variables(fragment: &mut Fragment, draft: &FileDraft) {
    for variable in &draft.variables {
        declare(
            fragment,
            Node::Module(draft.module.clone()),
            variable.symbol(&draft.file),
        );
        for param in &variable.params {
            declare(
                fragment,
                Node::Symbol(variable.id.clone()),
                param.symbol(&draft.file),
            );
        }
    }
}

fn index_snippets(
    env: &Env,
    analysis: &mut Analysis<'_>,
    fragment: &mut Fragment,
    draft: &FileDraft,
    output: &mut Indexed,
) {
    for snippet in &draft.snippets {
        if snippet.declared {
            declare(
                fragment,
                Node::Module(draft.module.clone()),
                snippet.symbol(&draft.file),
            );
        }
        match env.compile(&snippet.source.text) {
            Ok(program) => {
                let (summary, edges) = analysis.run(
                    &program,
                    &snippet.source,
                    &snippet.id,
                    &snippet.doc_id,
                    &snippet.params,
                );
                fragment.functions.push(summary);
                fragment.edges.extend(edges);
            }
            Err(error) => {
                let at = snippet
                    .source
                    .span(0, snippet.source.text.len(), &snippet.source.host)
                    .start;
                output.incomplete.push(Incomplete {
                    path: Some(draft.file.path.clone()),
                    reason: format!(
                        "CEL expression at {}:{} does not parse: {error}",
                        at.line, at.col
                    ),
                });
            }
        }
    }
}

fn declare(fragment: &mut Fragment, from: Node, symbol: Symbol) {
    fragment.edges.push(Edge {
        kind: EdgeKind::Contains,
        from,
        to: Target::Path(symbol.id.as_str().to_owned()),
        resolution: Resolution::Syntactic,
        site: Some(symbol.span),
    });
    fragment.symbols.push(symbol);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lighthouse_model::ucm::File;
    use lighthouse_plugin::{EmbeddedSource, Source, Workspace};
    use serde_json::json;

    use super::*;

    #[test]
    fn indexes_embedded_cel_at_original_host_coordinates() {
        let host = "where: node.name != \"\"\n";
        let expression = "node.name != \"\"";
        let start = host.find(expression).unwrap();
        let file = File {
            path: PathBuf::from("decisions/example.yaml"),
            lang: "yaml".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: Default::default(),
        };
        let embedded = EmbeddedFile {
            file,
            text: host.to_owned(),
            sources: vec![EmbeddedSource {
                id: "decision-where:decisions/example.yaml:0:$.where".to_owned(),
                rule_id: "decision-where".to_owned(),
                node_pointer: "/where".to_owned(),
                source_pointer: "/where".to_owned(),
                provider: "cel".to_owned(),
                syntax: None,
                context: json!({"identity":{"kind":"Decision","name":"example/check"}, "declared":true}),
                text: expression.to_owned(),
                source_range: [start as u32, (start + expression.len()) as u32],
                text_map: Some(
                    expression
                        .char_indices()
                        .map(|(offset, ch)| lighthouse_plugin::TextMapSegment {
                            text_range: [offset as u32, (offset + ch.len_utf8()) as u32],
                            source_range: [
                                (start + offset) as u32,
                                (start + offset + ch.len_utf8()) as u32,
                            ],
                        })
                        .collect(),
                ),
            }],
        };
        let source = Source {
            file: &embedded.file,
            text: &embedded.text,
            regions: &embedded.sources,
        };
        let indexed = Language::new()
            .index(&Workspace::new("/tmp"), &[source])
            .unwrap();
        assert!(indexed.incomplete.is_empty(), "{:?}", indexed.incomplete);
        let project = lighthouse_model::ucm::Project::merge(indexed.fragments);
        assert_eq!(project.files.len(), 1);
        assert_eq!(project.files[0].lang, "yaml");
        let expression_symbol = project
            .symbols
            .iter()
            .find(|symbol| symbol.name == "where")
            .unwrap();
        assert_eq!(expression_symbol.span.start.line, 1);
        assert_eq!(expression_symbol.span.start.col, 8);
        assert!(
            project
                .functions
                .iter()
                .any(|function| function.statements > 0)
        );

        let mut unsupported = embedded.clone();
        unsupported.sources[0].syntax = Some("sql".to_owned());
        let source = Source {
            file: &unsupported.file,
            text: &unsupported.text,
            regions: &unsupported.sources,
        };
        let indexed = Language::new()
            .index(&Workspace::new("/tmp"), &[source])
            .unwrap();
        assert!(
            indexed
                .incomplete
                .iter()
                .any(|gap| gap.reason.contains("does not support embedded syntax sql")),
            "{:?}",
            indexed.incomplete
        );
        assert!(
            indexed
                .fragments
                .iter()
                .all(|fragment| fragment.functions.is_empty()),
            "unsupported syntax must not be analyzed as direct CEL"
        );
    }

    #[test]
    fn template_syntax_rejects_unclosed_or_unmatched_delimiters() {
        assert_eq!(template_parts("hello {{ value }}!").unwrap().len(), 1);
        assert!(template_parts("hello {{ value").is_err());
        assert!(template_parts("hello value }}").is_err());
        assert!(template_parts("{{ }}").is_err());
    }
}
