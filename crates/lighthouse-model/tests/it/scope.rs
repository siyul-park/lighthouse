use lighthouse_model::{
    ExampleScope, Position, Span, TestScope,
    ucm::{
        Applicability, Edge, EdgeKind, File, Fragment, Module, Node, Project, Resolution, Symbol,
        SymbolId, SymbolKind, Target, Visibility,
    },
};

fn file(path: &str, generated: bool, test: bool) -> File {
    File {
        path: path.into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated,
        test,
        facts: Default::default(),
    }
}

fn symbol(path: &str, name: &str) -> Symbol {
    let p = Position { line: 1, col: 1 };
    Symbol {
        id: SymbolId::new("m", &[], name, SymbolKind::Function),
        kind: SymbolKind::Function,
        visibility: Visibility::Public,
        owner: None,
        file: path.into(),
        span: Span { start: p, end: p },
        extent: None,
        doc: None,
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

fn by(generated: bool, tests: TestScope) -> Applicability {
    Applicability {
        generated,
        tests,
        ..Applicability::default()
    }
}

#[test]
fn test_scope() {
    assert_eq!(TestScope::default(), TestScope::Exclude);
    assert_eq!(serde_json::to_value(TestScope::Only).unwrap(), "only");
    let parsed: TestScope = serde_json::from_value("include".into()).unwrap();
    assert_eq!(parsed, TestScope::Include);
}

#[test]
fn test_scope_is_default() {
    assert!(TestScope::Exclude.is_default());
    assert!(!TestScope::Include.is_default());
    assert!(!TestScope::Only.is_default());
}

#[test]
fn example_scope() {
    assert_eq!(ExampleScope::default(), ExampleScope::Include);
    assert_eq!(serde_json::to_value(ExampleScope::Only).unwrap(), "only");
    let parsed: ExampleScope = serde_json::from_value("exclude".into()).unwrap();
    assert_eq!(parsed, ExampleScope::Exclude);
}

#[test]
fn example_scope_is_default() {
    assert!(ExampleScope::Include.is_default());
    assert!(!ExampleScope::Exclude.is_default());
    assert!(!ExampleScope::Only.is_default());
}

#[test]
fn example_scope_admits() {
    assert!(ExampleScope::Exclude.admits(false));
    assert!(!ExampleScope::Exclude.admits(true));
    assert!(ExampleScope::Include.admits(false));
    assert!(ExampleScope::Include.admits(true));
    assert!(!ExampleScope::Only.admits(false));
    assert!(ExampleScope::Only.admits(true));
}

#[test]
fn applicability() {
    let standard = Applicability::default();

    assert!(!standard.generated);
    assert_eq!(standard.tests, TestScope::Exclude);
}

#[test]
fn applicability_excludes_files_without_subjects() {
    let production = file("a.go", false, false);
    let generated = file("g.go", true, false);
    let test = file("a_test.go", false, true);

    let none = Project::default();
    let standard = Applicability::default();
    assert!(!standard.excludes(&none, &production));
    assert!(standard.excludes(&none, &generated));
    assert!(standard.excludes(&none, &test));

    assert!(!by(true, TestScope::Include).excludes(&none, &generated));
    // `only` drops a production file that holds no test code.
    assert!(by(true, TestScope::Only).excludes(&none, &production));
}

#[test]
fn applicability_excludes() {
    let plain = symbol("a.go", "Plain");
    let inline = symbol("lib.go", "InlineTest");
    let project = Project::merge([Fragment {
        files: vec![file("a.go", false, false), file("lib.go", false, false)],
        modules: vec![Module {
            path: "m".to_owned(),
            name: None,
            test_of: Some("n".to_owned()),
            language: None,
            example: false,
        }],
        symbols: vec![plain.clone(), inline.clone()],
        ..Fragment::default()
    }]);
    let only = by(false, TestScope::Only);
    let production = by(false, TestScope::Exclude);
    let silent = file("quiet.go", false, false);

    assert!(only.excludes(&project, &silent), "no test code in it");
    assert!(!only.excludes(&project, &file("a_test.go", false, true)));
    assert!(!production.excludes(&project, &silent));
    assert!(production.excludes(&project, &file("a_test.go", false, true)));
}

#[test]
fn applicability_admits_file() {
    let production = file("a.go", false, false);
    let generated = file("g.go", true, false);
    let test = file("a_test.go", false, true);

    assert!(by(false, TestScope::Exclude).admits_file(&production));
    assert!(!by(false, TestScope::Exclude).admits_file(&test));
    assert!(!by(false, TestScope::Exclude).admits_file(&generated));
    assert!(by(true, TestScope::Exclude).admits_file(&generated));
    assert!(by(false, TestScope::Only).admits_file(&test));
    assert!(!by(false, TestScope::Only).admits_file(&production));
    assert!(by(false, TestScope::Include).admits_file(&test));
}

#[test]
fn applicability_admits_comment() {
    let production = file("a.go", false, false);
    let generated = file("g.go", true, false);

    let standard = Applicability::default();
    assert!(standard.admits_comment(Some(&production)));
    assert!(!standard.admits_comment(Some(&generated)));
    assert!(
        !standard.admits_comment(None),
        "an unknown file counts as generated"
    );
    assert!(by(true, TestScope::Exclude).admits_comment(Some(&generated)));
}

#[test]
fn applicability_admits_module() {
    let module = |test_of: Option<&str>| Module {
        path: "m".to_owned(),
        name: None,
        test_of: test_of.map(str::to_owned),
        language: None,
        example: false,
    };
    let prod = file("a.go", false, false);
    let generated = file("g.go", true, false);
    let tests = file("a_test.go", false, true);

    let standard = Applicability::default();
    assert!(standard.admits_module(&module(None), &[&prod]));
    assert!(!standard.admits_module(&module(Some("n")), &[&prod]));
    assert!(!standard.admits_module(&module(None), &[&tests]));
    assert!(!standard.admits_module(&module(None), &[&generated]));
    assert!(standard.admits_module(&module(None), &[&generated, &prod]));
    assert!(by(false, TestScope::Only).admits_module(&module(Some("n")), &[&prod]));
    assert!(by(true, TestScope::Include).admits_module(&module(None), &[&generated]));
}

#[test]
fn applicability_admits_symbol() {
    let plain = symbol("a.go", "Plain");
    let in_test = symbol("a_test.go", "Helper");
    let in_generated = symbol("g.go", "Gen");
    let project = Project::merge([Fragment {
        files: vec![
            file("a.go", false, false),
            file("a_test.go", false, true),
            file("g.go", true, false),
        ],
        symbols: vec![plain.clone(), in_test.clone(), in_generated.clone()],
        ..Fragment::default()
    }]);

    let production = by(false, TestScope::Exclude);
    assert!(production.admits_symbol(&project, &plain));
    assert!(!production.admits_symbol(&project, &in_test));
    assert!(!production.admits_symbol(&project, &in_generated));
    let tests = by(false, TestScope::Only);
    assert!(tests.admits_symbol(&project, &in_test));
    assert!(!tests.admits_symbol(&project, &plain));
    assert!(by(true, TestScope::Include).admits_symbol(&project, &in_generated));
}

#[test]
fn applicability_admits_only_its_languages() {
    let go = file("a.go", false, false);
    let yaml = File {
        lang: "yaml".to_owned(),
        ..file("a.yaml", false, false)
    };
    let cel = File {
        facts: [("analysisLanguage".to_owned(), "cel".to_owned())].into(),
        ..yaml.clone()
    };
    let project = Project::merge([Fragment {
        files: vec![go.clone(), yaml.clone(), cel.clone()],
        symbols: vec![symbol("a.go", "InGo")],
        ..Fragment::default()
    }]);
    let module = Module {
        path: "m".to_owned(),
        name: None,
        test_of: None,
        language: None,
        example: false,
    };
    let any = Applicability::default();
    let go_only = Applicability::default().only(["go".to_owned()]);
    let none = Applicability::default().only([]);

    assert!(any.admits_language("cel") && none.admits_language("cel"));
    assert!(go_only.admits_language("go") && !go_only.admits_language("yaml"));
    assert!(go_only.admits_file(&go) && !go_only.admits_file(&yaml));
    assert!(!go_only.admits_file(&cel), "the analyzed language counts");
    assert!(go_only.excludes(&project, &yaml) && !go_only.excludes(&project, &go));
    assert!(go_only.admits_comment(Some(&go)) && !go_only.admits_comment(Some(&yaml)));
    let host_toml_with_embedded_cel = File {
        path: "config.toml".into(),
        lang: "toml".to_owned(),
        facts: [("analysisLanguage".to_owned(), "cel".to_owned())].into(),
        ..file("config.toml", false, false)
    };
    let toml_only = Applicability::default().only(["toml".to_owned()]);
    assert!(toml_only.admits_file(&host_toml_with_embedded_cel));
    assert!(toml_only.admits_comment(Some(&host_toml_with_embedded_cel)));
    assert!(!go_only.admits_file(&host_toml_with_embedded_cel));
    assert!(go_only.admits_module(&module, &[&go, &yaml]));
    assert!(!go_only.admits_module(&module, &[&yaml]));
    assert!(go_only.admits_module(&module, &[]));
    let symbol = project
        .symbols_in(std::path::Path::new("a.go"))
        .next()
        .unwrap();
    assert!(go_only.admits_symbol(&project, symbol));
    assert!(
        !Applicability::default()
            .only(["rust".to_owned()])
            .admits_symbol(&project, symbol)
    );
}

fn in_module(module: &str, name: &str) -> Symbol {
    Symbol {
        id: SymbolId::new(module, &[], name, SymbolKind::Function),
        ..symbol("doc.md", name)
    }
}

/// A host file with a snippet module whose function calls a function of the
/// project.
fn with_example() -> (Project, Symbol, Symbol) {
    let used = symbol("a.go", "Used");
    let snippet = in_module("doc.0", "main");
    let project = Project::merge([Fragment {
        files: vec![
            file("a.go", false, false),
            File {
                lang: "markdown".to_owned(),
                facts: [("regions".to_owned(), "go:example".to_owned())].into(),
                ..file("doc.md", false, false)
            },
        ],
        modules: vec![Module {
            path: "doc.0".to_owned(),
            name: None,
            test_of: None,
            language: Some("go".to_owned()),
            example: true,
        }],
        symbols: vec![used.clone(), snippet.clone()],
        edges: vec![Edge {
            kind: EdgeKind::Calls,
            from: Node::Symbol(snippet.id.clone()),
            to: Target::Resolved(Node::Symbol(used.id.clone())),
            resolution: Resolution::Semantic,
            site: Some(Span {
                start: Position { line: 3, col: 2 },
                end: Position { line: 3, col: 6 },
            }),
        }],
        ..Fragment::default()
    }]);
    (project, used, snippet)
}

#[test]
fn an_example_is_a_site_of_use_and_no_caller() {
    let (project, used, snippet) = with_example();
    assert!(project.in_example(&snippet.id));
    assert!(!project.in_example(&used.id));
    assert!(project.callers(&used.id).is_empty());
    assert!(project.callees(&snippet.id).is_empty());
    assert!(project.uses(&snippet.id).is_empty());
    assert!(project.references(&used.id).is_empty());
    assert_eq!(project.sites(&used.id).len(), 1);
    assert_eq!(project.edges_from(&snippet.id).count(), 1);
}

#[test]
fn the_examples_scope_decides_whether_an_example_is_a_subject() {
    let (project, used, snippet) = with_example();
    let scope = |examples| Applicability {
        examples,
        ..Applicability::default()
    };
    assert_eq!(ExampleScope::default(), ExampleScope::Include);
    assert!(ExampleScope::Include.is_default());
    assert!(scope(ExampleScope::Include).admits_symbol(&project, &snippet));
    assert!(!scope(ExampleScope::Exclude).admits_symbol(&project, &snippet));
    assert!(scope(ExampleScope::Exclude).admits_symbol(&project, &used));
    assert!(scope(ExampleScope::Only).admits_symbol(&project, &snippet));
    assert!(!scope(ExampleScope::Only).admits_symbol(&project, &used));
}

#[test]
fn a_symbol_is_in_the_language_its_module_says() {
    let (project, used, snippet) = with_example();
    assert_eq!(project.language_of(&snippet), Some("go"));
    assert_eq!(project.language_of(&used), Some("go"));
    let rust = Applicability::default().only(["rust".to_owned()]);
    assert!(!rust.admits_symbol(&project, &snippet));
}

#[test]
fn a_host_file_is_a_subject_through_the_regions_it_holds() {
    let (project, _, _) = with_example();
    let host = project.file(std::path::Path::new("doc.md")).unwrap();
    assert_eq!(host.regions().collect::<Vec<_>>(), [("go", true)]);
    let go = Applicability::default().only(["go".to_owned()]);
    assert!(!go.excludes(&project, host));
    assert!(!go.admits_file(host), "the file itself is Markdown");
    let outside = Applicability {
        examples: ExampleScope::Exclude,
        ..go.clone()
    };
    assert!(outside.excludes(&project, host));
    let rust = Applicability::default().only(["rust".to_owned()]);
    assert!(rust.excludes(&project, host));
    let only = Applicability {
        examples: ExampleScope::Only,
        ..Applicability::default()
    };
    assert!(!only.excludes(&project, host));
    assert!(only.excludes(
        &project,
        project.file(std::path::Path::new("a.go")).unwrap()
    ));
}

#[test]
fn applicability_admits_an_example_module_as_the_scope_says() {
    let module = Module {
        path: "doc.0".to_owned(),
        name: None,
        test_of: None,
        language: Some("go".to_owned()),
        example: true,
    };
    let scope = |examples| Applicability {
        examples,
        ..Applicability::default()
    };
    assert!(scope(ExampleScope::Include).admits_module(&module, &[]));
    assert!(!scope(ExampleScope::Exclude).admits_module(&module, &[]));
    assert!(scope(ExampleScope::Only).admits_module(&module, &[]));
}
