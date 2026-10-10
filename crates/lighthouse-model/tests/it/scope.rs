use lighthouse_model::{
    Applicability, File, Fragment, Module, Position, Project, Span, Symbol, SymbolId, SymbolKind,
    TestScope, Visibility,
};

fn file(path: &str, generated: bool, test: bool) -> File {
    File {
        path: path.into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated,
        test,
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
    Applicability { generated, tests }
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
fn applicability() {
    let standard = Applicability::default();

    assert!(!standard.generated);
    assert_eq!(standard.tests, TestScope::Exclude);
}

#[test]
fn applicability_excludes_file() {
    let production = file("a.go", false, false);
    let generated = file("g.go", true, false);
    let test = file("a_test.go", false, true);

    let standard = Applicability::default();
    assert!(!standard.excludes_file(&production));
    assert!(standard.excludes_file(&generated));
    assert!(standard.excludes_file(&test));

    // `only` cannot drop a production file: its inline test modules may count.
    assert!(!by(true, TestScope::Only).excludes_file(&production));
    assert!(!by(true, TestScope::Include).excludes_file(&generated));
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
fn applicability_excludes() {
    let plain = symbol("a.go", "Plain");
    let inline = symbol("lib.go", "InlineTest");
    let project = Project::merge([Fragment {
        files: vec![file("a.go", false, false), file("lib.go", false, false)],
        modules: vec![Module {
            path: "m".to_owned(),
            name: None,
            test_of: Some("n".to_owned()),
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
fn applicability_admits_module() {
    let module = |test_of: Option<&str>| Module {
        path: "m".to_owned(),
        name: None,
        test_of: test_of.map(str::to_owned),
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
