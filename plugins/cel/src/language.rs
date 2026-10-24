use std::collections::{BTreeMap, BTreeSet};

use cel::Env;
use lighthouse_model::{
    Incomplete,
    ucm::{
        Capability, Edge, EdgeKind, Fragment, Module, Node, Resolution, Symbol, SymbolId, Target,
    },
};
use lighthouse_plugin::{Error, Indexed, LanguageProvider, ProviderManifest, Source, Workspace};
use serde::Deserialize;

mod analysis;
mod host;
#[cfg(test)]
mod tests;
mod toml_host;
mod yaml;

use analysis::{Analysis, PackVariables, external_module, function_catalog};
use host::FileDraft;
use toml_host::toml_draft;
use yaml::yaml_draft;

const EXPRESSION_FIELDS: [&str; 7] = [
    "select", "each", "tool", "path", "pattern", "command", "said",
];

pub struct Language {
    manifest: ProviderManifest,
}

impl Language {
    pub fn new() -> Self {
        let mut manifest = ProviderManifest::new(
            "cel",
            [
                "decisions/**/*.yaml",
                "decisions/**/*.yml",
                "packs/**/*.yaml",
                "packs/**/*.yml",
                ".lighthouse/**/*.yaml",
                ".lighthouse/**/*.yml",
                ".lighthouse/**/*.toml",
                "**/.lighthouse/**/*.yaml",
                "**/.lighthouse/**/*.yml",
                "**/.lighthouse/**/*.toml",
                "lighthouse.toml",
                "recipes/**/*.toml",
            ]
            .map(str::to_owned)
            .to_vec(),
        );
        manifest.capabilities = [
            Capability::SemanticEdges,
            Capability::Extent,
            Capability::ReferenceSites,
            Capability::CompleteReferences,
            Capability::Overlays,
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

    fn index(&self, workspace: &Workspace, sources: &[Source]) -> Result<Indexed, Error> {
        let env = Env::stdlib();
        let (drafts, mut output) = file_drafts(sources);
        let packs = pack_variables(&drafts);
        let (fragments, external) =
            self.index_drafts(&workspace.cel_libraries, &env, &drafts, &packs, &mut output);
        let modules: BTreeSet<String> = external.values().map(external_module).collect();
        output.fragments = fragments;
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
        Ok(output)
    }
}

impl Language {
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

fn file_drafts(sources: &[Source]) -> (Vec<FileDraft>, Indexed) {
    let mut output = Indexed::default();
    let drafts = sources
        .iter()
        .map(|source| match file_draft(source) {
            Ok(draft) => draft,
            Err(reason) => {
                output.incomplete.push(Incomplete {
                    path: Some(source.file.path.clone()),
                    reason,
                });
                FileDraft::empty(source)
            }
        })
        .collect();
    (drafts, output)
}

fn file_draft(source: &Source) -> Result<FileDraft, String> {
    let ext = source
        .file
        .path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if ext == "toml" {
        toml_draft(source).map_err(|reason| format!("{}: {reason}", source.file.path.display()))
    } else {
        let parsed: Result<Vec<serde_json::Value>, _> =
            serde_norway::Deserializer::from_str(source.text)
                .map(serde_json::Value::deserialize)
                .collect();
        parsed.map_err(|error| format!("{}: invalid YAML: {error}", source.file.path.display()))?;
        Ok(yaml_draft(source))
    }
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
