use std::path::{Component, Path};

use lighthouse_model::{Capability, File, Fragment, Module};
use lighthouse_plugin::{Conventions, Error, LanguageProvider, Workspace};
use lighthouse_syntax::{Query, Syntax};
use tree_sitter_go::LANGUAGE;

use crate::{
    body,
    declarations::{Declared, Site},
    imports::Resolver,
    testcase,
};

const ID: &str = "go";
const DECLARATIONS: &str = include_str!("../queries/declarations.scm");
const IMPORTS: &str = include_str!("../queries/imports.scm");
const IGNORED_DIRS: &[&str] = &["testdata", "vendor"];

pub struct Go {
    globs: Vec<String>,
    syntax: Syntax,
    declarations: Query,
    imports: Query,
    resolver: Resolver,
}

impl Go {
    pub fn new() -> Self {
        let mut syntax = Syntax::default();
        syntax.register(ID, LANGUAGE);
        let language = syntax.language(ID).expect("go grammar is registered");
        let declarations =
            Query::new(language, DECLARATIONS).expect("bundled declarations query is valid");
        let imports = Query::new(language, IMPORTS).expect("bundled imports query is valid");
        Self {
            globs: vec!["**/*.go".to_owned()],
            syntax,
            declarations,
            imports,
            resolver: Resolver::default(),
        }
    }
}

impl Default for Go {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageProvider for Go {
    fn id(&self) -> &str {
        ID
    }

    fn globs(&self) -> &[String] {
        &self.globs
    }

    fn conventions(&self) -> Conventions {
        Conventions {
            test_globs: vec!["**/*_test.go".to_owned()],
        }
    }

    fn capabilities(&self) -> &[Capability] {
        &[]
    }

    fn index(&self, ws: &Workspace, file: &File, text: &str) -> Result<Fragment, Error> {
        let mut file = file.clone();
        if ignored(&file.path) {
            return Ok(Fragment {
                files: vec![file],
                ..Fragment::default()
            });
        }
        let tree = self
            .syntax
            .parse(ID, text)
            .map_err(|e| Error::Failed(e.to_string()))?;
        let root = tree.root_node();
        if root.has_error() {
            return Err(Error::Failed(syntax_error(root)));
        }
        file.generated = generated(text);

        let decls = self.declarations.matches(root, text);
        let package = decls
            .iter()
            .find_map(|m| m.get("package"))
            .map(|n| lighthouse_syntax::text(n, text).to_owned())
            .ok_or_else(|| Error::Failed("no package clause".to_owned()))?;
        let dir = directory(&file.path);
        let test_file = file.path.to_string_lossy().ends_with("_test.go");
        let external = test_file && package.ends_with("_test");
        let module = if external {
            format!("{dir}[test]")
        } else {
            dir.clone()
        };
        let site = Site {
            module: &module,
            file: &file.path,
            source: text,
            test_file,
        };

        let specs = self.imports.matches(root, text);
        let imports = self.resolver.resolve(&ws.root, &dir, &module, &specs, text);
        let declared = Declared::collect(&site, &decls);

        let mut fragment = Fragment {
            modules: vec![Module {
                path: module.clone(),
                name: Some(package.clone()),
                test_of: external.then(|| dir.clone()),
            }],
            symbols: declared.symbols,
            edges: declared.edges,
            ..Fragment::default()
        };
        fragment.edges.extend(imports.edges);
        for function in &declared.functions {
            let analysis = body::analyze(&site, &imports.aliases, function);
            if function.node.child_by_field_name("body").is_some() {
                fragment.functions.push(analysis.summary);
            }
            fragment.edges.extend(analysis.edges);
            fragment
                .tests
                .extend(testcase::case(function, text, analysis.targets));
        }
        fragment.files = vec![file];
        Ok(fragment)
    }
}

fn ignored(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            IGNORED_DIRS.contains(&name.as_ref()) || name.starts_with(['_', '.'])
        }
        _ => false,
    })
}

fn directory(path: &Path) -> String {
    let dir = path
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if dir.is_empty() { ".".to_owned() } else { dir }
}

fn generated(source: &str) -> bool {
    source
        .lines()
        .take_while(|l| !l.starts_with("package "))
        .any(|l| l.starts_with("// Code generated ") && l.trim_end().ends_with("DO NOT EDIT."))
}

fn syntax_error(root: lighthouse_syntax::Node) -> String {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            return format!("syntax error at line {}", node.start_position().row + 1);
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    "syntax error".to_owned()
}
