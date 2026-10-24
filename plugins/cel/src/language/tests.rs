use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use lighthouse_model::ucm::{
    Edge, EdgeKind, File, FlowKind, Node, Project, Symbol, SymbolKind, Target, Visibility,
};
use lighthouse_plugin::{
    CelLibrary, CelLibraryManifest, Error as PluginError, Function, Indexed, Overload, Plugin,
    PluginManifest, Precedence, Registry, Source, Workspace,
};

use crate::Cel;

fn index(files: &[(&str, &str)]) -> Indexed {
    let mut registry = Registry::default();
    registry.register(&Cel::new()).unwrap();
    index_with_registry(&registry, files)
}

fn index_with_registry(registry: &Registry, files: &[(&str, &str)]) -> Indexed {
    let inputs: Vec<File> = files
        .iter()
        .map(|(path, _)| File {
            path: PathBuf::from(path),
            lang: "cel".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: BTreeMap::new(),
        })
        .collect();
    let sources: Vec<Source<'_>> = inputs
        .iter()
        .zip(files)
        .map(|(file, (_, text))| Source { file, text })
        .collect();
    let language = registry
        .languages()
        .iter()
        .find(|language| language.provider.manifest().id == "cel")
        .unwrap();
    let mut workspace = Workspace::new("/tmp");
    workspace.cel_libraries = registry
        .cel()
        .libraries()
        .map(|library| library.manifest().clone())
        .collect();
    language.provider.index(&workspace, &sources).unwrap()
}

fn symbol<'a>(project: &'a Project, file: &str, name: &str, kind: SymbolKind) -> &'a Symbol {
    project
        .symbols
        .iter()
        .find(|symbol| symbol.file == Path::new(file) && symbol.name == name && symbol.kind == kind)
        .unwrap_or_else(|| panic!("missing {kind:?} symbol {name} in {file}"))
}

fn resolved_to(edge: &Edge, symbol: &Symbol) -> bool {
    edge.to == Target::Resolved(Node::Symbol(symbol.id.clone()))
}

#[test]
fn decision_and_pack_expressions_form_one_reference_graph_with_host_spans() {
    let pack = r#"---
apiVersion: lighthouse/v1alpha1
kind: Pack
metadata:
  name: sample
spec:
  variables:
    - name: base
      expression: |
        size(node.children)
"#;
    let other_pack = r#"apiVersion: lighthouse/v1alpha1
kind: Pack
metadata:
  name: other
spec:
  variables:
    - name: base
      expression: 999
"#;
    let decision = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: sample/check
spec:
  check:
    type: cel
    variables:
      - name: local
        expression: >-
          base + 1
      - name: alias
        expression: base
      - name: predicate
        params:
          - "item"
        expression: >-
          item > local
    where: >-
      predicate(node.children) && local > 0 && base > 0
    message: "found {{ local }}"
    evidence:
      count: "local"
"#;
    let result = index(&[
        ("decisions/design/functions/pack.yaml", pack),
        ("decisions/design/other/pack.yaml", other_pack),
        ("decisions/design/functions/check.yaml", decision),
    ]);
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let project = Project::merge(result.fragments);

    let base = symbol(
        &project,
        "decisions/design/functions/pack.yaml",
        "base",
        SymbolKind::Var,
    );
    assert_eq!(base.visibility, Visibility::Public);
    let pack_line = pack
        .lines()
        .position(|line| line.contains("size(node.children)"))
        .unwrap() as u32
        + 1;
    let pack_col = pack
        .lines()
        .find(|line| line.contains("size(node.children)"))
        .unwrap()
        .find("size")
        .unwrap() as u32
        + 1;
    assert!(
        project.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Calls
                && edge.from == Node::Symbol(base.id.clone())
                && edge
                    .site
                    .is_some_and(|site| site.start.line == pack_line && site.start.col == pack_col)
        }),
        "literal block scalar call must map to {pack_line}:{pack_col}"
    );
    let local = symbol(
        &project,
        "decisions/design/functions/check.yaml",
        "local",
        SymbolKind::Var,
    );
    let alias = symbol(
        &project,
        "decisions/design/functions/check.yaml",
        "alias",
        SymbolKind::Var,
    );
    let predicate = symbol(
        &project,
        "decisions/design/functions/check.yaml",
        "predicate",
        SymbolKind::Var,
    );
    let param = symbol(
        &project,
        "decisions/design/functions/check.yaml",
        "item",
        SymbolKind::Var,
    );
    assert_eq!(param.owner.as_ref(), Some(&predicate.id));

    assert!(project.edges.iter().any(|edge| {
        edge.kind == EdgeKind::References
            && edge.from == Node::Symbol(local.id.clone())
            && resolved_to(edge, base)
    }));
    assert!(
        project.edges.iter().any(|edge| {
            edge.kind == EdgeKind::References
                && edge.from == Node::Symbol(predicate.id.clone())
                && resolved_to(edge, param)
        }),
        "param {:?}; predicate edges {:?}",
        param.id,
        project
            .edges
            .iter()
            .filter(|edge| edge.from == Node::Symbol(predicate.id.clone()))
            .collect::<Vec<_>>()
    );
    assert!(project.edges.iter().any(|edge| {
        edge.kind == EdgeKind::References
            && edge.from == Node::Symbol(predicate.id.clone())
            && resolved_to(edge, local)
    }));

    let where_expr = symbol(
        &project,
        "decisions/design/functions/check.yaml",
        "where",
        SymbolKind::Function,
    );
    assert!(
        project
            .functions
            .iter()
            .any(|function| function.symbol == predicate.id)
    );
    assert!(project.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Calls
            && edge.from == Node::Symbol(where_expr.id.clone())
            && resolved_to(edge, predicate)
    }));
    let expected_line = decision
        .lines()
        .position(|line| line.contains("predicate(node.children)"))
        .unwrap() as u32
        + 1;
    let expected_col = decision
        .lines()
        .find(|line| line.contains("predicate(node.children)"))
        .unwrap()
        .find("predicate")
        .unwrap() as u32
        + 1;
    assert_eq!(where_expr.span.start.line, expected_line);
    assert_eq!(where_expr.span.start.col, expected_col);
    assert!(
        project.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Calls
                && edge.from == Node::Symbol(where_expr.id.clone())
                && resolved_to(edge, predicate)
                && edge.site.is_some_and(|site| {
                    site.start.line == expected_line && site.start.col == expected_col
                })
        }),
        "expected call at {expected_line}:{expected_col}; calls: {:?}",
        project
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Calls)
            .map(|edge| (&edge.from, &edge.to, edge.site))
            .collect::<Vec<_>>()
    );

    let alias_summary = project
        .functions
        .iter()
        .find(|function| function.symbol == alias.id)
        .expect("variable expressions carry summaries");
    assert_eq!(
        alias_summary.forwards_to,
        Some(Target::Resolved(Node::Symbol(base.id.clone())))
    );

    let summary = project
        .functions
        .iter()
        .find(|function| function.symbol == where_expr.id)
        .unwrap();
    assert!(
        summary.max_nesting == 1,
        "a flat logical chain has one control-flow nesting level"
    );
    assert!(
        summary.tokens > summary.statements,
        "token count must not alias AST node count"
    );
    let logic: Vec<_> = summary
        .flow
        .iter()
        .filter(|flow| flow.kind == FlowKind::Logic)
        .collect();
    assert_eq!(logic.len(), 1, "same-operator logic runs form one flow");
    assert_eq!(logic[0].operators, 2);
}

#[test]
fn indexes_toml_event_expressions_optional_syntax_and_comprehensions() {
    let recipe = r#"[models.providers.agent.events]
select = 'line.?result.orValue("").size() > 0'
each = 'line.actions.filter(action, action.ok)'
tool = """
  tools[item.name] == \"read\" &&
  \"한글\" == line.part.state.input.?pattern.orValue(\"\")
"""
path = "\"한글\" == line.part.state.input.?pattern.orValue(\"\")"
pattern = '''
  line.part.state.input.?pattern.orValue("")
'''
tools = { Read = "read" }
"#;
    let project_config = r#"[spec.models.providers.agent.events]
select = 'node.name != ""'
"#;
    let result = index(&[
        ("recipes/agent/recipe.toml", recipe),
        ("lighthouse.toml", project_config),
    ]);
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let project = Project::merge(result.fragments);
    let file = Path::new("recipes/agent/recipe.toml");
    for name in ["select", "each", "tool", "path", "pattern"] {
        assert!(
            project.symbols.iter().any(|symbol| {
                symbol.file == file && symbol.kind == SymbolKind::Function && symbol.name == name
            }),
            "missing TOML CEL expression {name}"
        );
    }
    assert!(
        project.symbols.iter().any(|symbol| {
            symbol.file == Path::new("lighthouse.toml")
                && symbol.kind == SymbolKind::Function
                && symbol.name == "select"
        }),
        "the spec-prefixed project events table must be indexed"
    );
    let select = symbol(
        &project,
        "recipes/agent/recipe.toml",
        "select",
        SymbolKind::Function,
    );
    let select_line = recipe
        .lines()
        .position(|line| line.starts_with("select = "))
        .unwrap() as u32
        + 1;
    let select_col = recipe
        .lines()
        .find(|line| line.starts_with("select = "))
        .unwrap()
        .find("'line")
        .unwrap() as u32
        + 2;
    assert_eq!(select.span.start.line, select_line);
    assert_eq!(select.span.start.col, select_col);
    let pattern_source = recipe
        .lines()
        .find(|line| line.trim_start().starts_with("line.part.state.input"))
        .unwrap();
    let pattern_line = recipe
        .lines()
        .position(|line| line.trim_start().starts_with("line.part.state.input"))
        .unwrap() as u32
        + 1;
    let pattern = symbol(
        &project,
        "recipes/agent/recipe.toml",
        "pattern",
        SymbolKind::Function,
    );
    assert_eq!(pattern.span.start.line, pattern_line);
    for (line, source_line) in [
        (
            recipe
                .lines()
                .position(|line| {
                    line.contains("line.part.state.input") && !line.starts_with("path = ")
                })
                .unwrap() as u32
                + 1,
            recipe
                .lines()
                .find(|line| line.contains("line.part.state.input") && !line.starts_with("path = "))
                .unwrap(),
        ),
        (
            recipe
                .lines()
                .position(|line| line.starts_with("path = "))
                .unwrap() as u32
                + 1,
            recipe
                .lines()
                .find(|line| line.starts_with("path = "))
                .unwrap(),
        ),
    ] {
        let column = source_line.find("orValue").unwrap() as u32 + 1;
        assert!(
            project.edges.iter().any(|edge| {
                edge.kind == EdgeKind::Calls
                    && edge
                        .site
                        .is_some_and(|site| site.start.line == line && site.start.col == column)
            }),
            "escaped/unicode TOML CEL call should map to {line}:{column}"
        );
    }
    assert!(
        project.edges.iter().any(|edge| {
            edge.kind == EdgeKind::References
                && edge
                    .site
                    .is_some_and(|site| site.start.line == pattern_line && site.start.col == 8)
        }),
        "the multiline TOML mapping should retain the exact CEL field span"
    );
    let call_col = pattern_source.find("orValue").unwrap() as u32 + 1;
    assert!(
        project.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Calls
                && edge.site.is_some_and(|site| {
                    site.start.line == pattern_line && site.start.col == call_col
                })
        }),
        "the multiline TOML mapping should retain the exact CEL call span"
    );
    assert!(
        project
            .functions
            .iter()
            .any(|function| { function.flow.iter().any(|flow| flow.kind == FlowKind::Loop) })
    );
    assert!(project.edges.iter().any(|edge| {
        edge.kind == EdgeKind::References && edge.site.is_some_and(|site| site.start.line == 5)
    }));
}

#[test]
fn library_calls_resolve_to_the_registered_catalog() {
    let decision = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: sample/check
spec:
  check:
    type: cel
    where: glob.matches("a.go", "**/*.go")
    message: "match"
"#;
    let result = index(&[("decisions/design/functions/check.yaml", decision)]);
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let catalog = result
        .fragments
        .iter()
        .find(|fragment| {
            fragment
                .symbols
                .iter()
                .any(|symbol| symbol.file == Path::new("@cel/library"))
        })
        .expect("the external CEL catalog has its own fragment");
    assert!(catalog.files.is_empty());
    assert!(
        catalog
            .symbols
            .iter()
            .all(|symbol| symbol.file == Path::new("@cel/library"))
    );
    assert!(
        result
            .fragments
            .iter()
            .filter(|fragment| !fragment.files.is_empty())
            .all(|fragment| !fragment
                .symbols
                .iter()
                .any(|symbol| symbol.file == Path::new("@cel/library")))
    );
    for (name, module) in [
        ("size", "cel/stdlib"),
        ("upperAscii", "cel/stdlib/member"),
        ("round", "cel/stdlib/math"),
    ] {
        assert!(
            catalog.symbols.iter().any(|symbol| {
                symbol.name == name
                    && symbol.kind == SymbolKind::Function
                    && symbol.id.module() == module
            }),
            "unused standard function {module}::{name} must still be cataloged"
        );
    }
    let project = Project::merge(result.fragments);
    let function = project
        .symbols
        .iter()
        .find(|symbol| {
            symbol.name == "matches"
                && symbol.kind == SymbolKind::Function
                && symbol.id.as_str().starts_with("cel/library/cel/glob::")
        })
        .unwrap_or_else(|| panic!("missing glob.matches: {:?}", project.symbols));
    assert!(
        project
            .edges
            .iter()
            .any(|edge| { edge.kind == EdgeKind::Calls && resolved_to(edge, function) })
    );
    assert!(
        project
            .modules
            .iter()
            .any(|module| { module.path == "cel/library/cel/glob" })
    );
    assert!(
        project.symbols.iter().any(|symbol| {
            symbol.name == "extract" && symbol.id.as_str().starts_with("cel/library/cel/regex::")
        }),
        "unreferenced library catalog functions must still be declared"
    );
}

#[test]
fn global_catalog_is_independent_of_host_file_order() {
    let first = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: sample/glob
spec:
  check:
    type: cel
    where: glob.matches("a.go", "**/*.go")
    message: "match"
"#;
    let second = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: sample/size
spec:
  check:
    type: cel
    where: size(node.children) > 0
    message: "has children"
"#;
    let forward = index(&[
        ("decisions/sample/a.yaml", first),
        ("decisions/sample/b.yaml", second),
    ]);
    let reverse = index(&[
        ("decisions/sample/b.yaml", second),
        ("decisions/sample/a.yaml", first),
    ]);
    assert!(forward.incomplete.is_empty(), "{:?}", forward.incomplete);
    assert!(reverse.incomplete.is_empty(), "{:?}", reverse.incomplete);
    let forward_catalog = forward
        .fragments
        .iter()
        .find(|fragment| {
            fragment.files.is_empty()
                && fragment
                    .symbols
                    .iter()
                    .any(|symbol| symbol.file == Path::new("@cel/library"))
        })
        .expect("the global catalog has a separate fragment");
    let reverse_catalog = reverse
        .fragments
        .iter()
        .find(|fragment| {
            fragment.files.is_empty()
                && fragment
                    .symbols
                    .iter()
                    .any(|symbol| symbol.file == Path::new("@cel/library"))
        })
        .expect("the global catalog has a separate fragment");
    assert_eq!(forward_catalog.symbols, reverse_catalog.symbols);
    assert_eq!(forward_catalog.modules, reverse_catalog.modules);
}

#[test]
fn standard_calls_resolve_and_unknown_names_do_not_become_fake_symbols() {
    let source = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: sample/check
spec:
  check:
    type: cel
    where: edge.kind != "" && decision.id != "" && node.name.lowerAscii().endsWith(".go") && missingFlag && unknownCall(node.path)
    message: "check"
"#;
    let result = index(&[("decisions/design/functions/check.yaml", source)]);
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let project = Project::merge(result.fragments);

    for name in ["lowerAscii", "endsWith"] {
        assert!(
            project.symbols.iter().any(|symbol| {
                symbol.name == name
                    && symbol.kind == SymbolKind::Function
                    && symbol.file == Path::new("@cel/library")
            }),
            "standard member {name} was not cataloged"
        );
    }
    assert!(
        !project
            .symbols
            .iter()
            .any(|symbol| { ["missingFlag", "unknownCall"].contains(&symbol.name.as_str()) })
    );
    assert!(!project.edges.iter().any(|edge| {
        matches!(&edge.to, Target::Path(path) if path.starts_with("cel/unresolved::edge") || path.starts_with("cel/unresolved::decision"))
    }), "built-in context variables must not be marked unresolved");
    assert!(project.edges.iter().any(|edge| {
        edge.kind == EdgeKind::References
            && matches!(&edge.to, Target::Path(path) if path.starts_with("cel/unresolved::missingFlag"))
    }));
    assert!(project.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Calls
            && matches!(&edge.to, Target::Path(path) if path.starts_with("cel/unresolved::unknownCall"))
    }));
}

static EXTENSION: LazyLock<CelLibraryManifest> = LazyLock::new(|| CelLibraryManifest {
    id: "extension/functions".to_owned(),
    version: "1".to_owned(),
    functions: vec![Function {
        name: "extension.isReady".to_owned(),
        receiver: None,
        overloads: vec![Overload::new(&["string"], "bool")],
        doc: "Checks readiness.".to_owned(),
        example: "extension.isReady('ready')".to_owned(),
        precedence: Precedence::Designed,
    }],
});

static EXTENSION_PLUGIN: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
    id: "extension".to_owned(),
    version: "1".to_owned(),
});

struct Extension;

impl Plugin for Extension {
    fn manifest(&self) -> &PluginManifest {
        &EXTENSION_PLUGIN
    }

    fn cel_libraries(&self) -> Vec<Box<dyn CelLibrary>> {
        vec![Box::new(ExtensionLibrary)]
    }
}

struct ExtensionLibrary;

impl CelLibrary for ExtensionLibrary {
    fn manifest(&self) -> &CelLibraryManifest {
        &EXTENSION
    }

    fn register(&self, _: &mut cel::Env) -> Result<(), PluginError> {
        Ok(())
    }
}

#[test]
fn language_receives_the_registry_catalog_from_other_plugins() {
    let mut registry = Registry::default();
    registry.register(&Cel::new()).unwrap();
    registry.register(&Extension).unwrap();
    let source = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: sample/catalog
spec:
  check:
    type: cel
    where: extension.isReady(node.name)
    message: "ready"
"#;
    let result = index_with_registry(&registry, &[("decisions/sample/catalog.yaml", source)]);
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let project = Project::merge(result.fragments);
    let function = project
        .symbols
        .iter()
        .find(|symbol| {
            symbol.name == "isReady"
                && symbol
                    .id
                    .as_str()
                    .starts_with("cel/library/extension/functions::")
        })
        .expect("an external library becomes a public language symbol");
    assert_eq!(function.visibility, Visibility::Public);
    assert_eq!(function.id.module(), "cel/library/extension/functions");
    assert!(
        project
            .edges
            .iter()
            .any(|edge| { edge.kind == EdgeKind::Calls && resolved_to(edge, function) })
    );
    assert!(
        project
            .modules
            .iter()
            .any(|module| { module.path == "cel/library/extension/functions" })
    );
}

#[test]
fn metrics_count_control_nesting_and_optional_calls_resolve() {
    let source = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: sample/nesting
spec:
  check:
    type: cel
    where: node.active && (node.result.?value.orValue(false) || optional.of(true).hasValue())
    message: "nested"
"#;
    let result = index(&[("decisions/sample/nesting.yaml", source)]);
    assert!(result.incomplete.is_empty(), "{:?}", result.incomplete);
    let project = Project::merge(result.fragments);
    let where_symbol = symbol(
        &project,
        "decisions/sample/nesting.yaml",
        "where",
        SymbolKind::Function,
    );
    let summary = project
        .functions
        .iter()
        .find(|function| function.symbol == where_symbol.id)
        .unwrap();
    assert_eq!(summary.max_nesting, 2);
    for name in ["orValue", "hasValue", "of"] {
        assert!(
            project.symbols.iter().any(|symbol| {
                symbol.name == name
                    && symbol.kind == SymbolKind::Function
                    && symbol.file == Path::new("@cel/library")
            }),
            "standard optional function {name} should resolve"
        );
    }
}
