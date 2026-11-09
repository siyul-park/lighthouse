//! The vocabulary the ordering checks share.

use lighthouse_checks::layout::{
    MODULE_BODY, declarations, declarations_in, exposed, has_word_prefix, is_declaration, owner_key,
};
use lighthouse_model::{
    Position, Span,
    ucm::{File, Fragment, Place, Project, Symbol, SymbolId, SymbolKind, SymbolRole, Visibility},
};
use lighthouse_plugin::{Context, Text, Workspace};

fn at(line: u32) -> Span {
    let p = Position { line, col: 1 };
    Span { start: p, end: p }
}

/// The symbols and their places, each declared on the line given.
fn placed(declared: &[(&Symbol, u32)]) -> (Vec<Symbol>, Vec<Place>) {
    declared
        .iter()
        .map(|(symbol, line)| ((*symbol).clone(), Place::new(symbol.id.clone(), at(*line))))
        .unzip()
}

fn symbol(name: &str, kind: SymbolKind, owner: Option<&Symbol>, _line: u32) -> Symbol {
    let owners: Vec<&str> = owner.iter().map(|o| o.name.as_str()).collect();
    Symbol {
        id: SymbolId::new("m", &owners, name, kind),
        kind,
        visibility: Visibility::Public,
        owner: owner.map(|o| o.id.clone()),
        file: "m/a.go".into(),
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
        implementation: false,
    }
}

fn file() -> File {
    File {
        path: "m/a.go".into(),
        lang: "go".to_owned(),
        generated: false,
        test: false,
        facts: Default::default(),
    }
}

#[test]
fn declarations_lists_each_module_of_a_file_in_source_order() {
    let ty = symbol("Store", SymbolKind::Type, None, 3);
    let method = symbol("Get", SymbolKind::Method, Some(&ty), 9);
    let func = symbol("Open", SymbolKind::Function, None, 5);
    let field = symbol("id", SymbolKind::Field, Some(&ty), 4);
    let file = file();
    let (symbols, places) = placed(&[(&method, 9), (&func, 5), (&field, 4), (&ty, 3)]);
    let project = Project::merge([Fragment {
        files: vec![file.clone()],
        symbols,
        places,
        ..Fragment::default()
    }]);
    let ws = Workspace::new(".");
    let facts = Default::default();
    let given = Text::given("");
    let ctx = Context {
        workspace: &ws,
        model: &project,
        file: Some((&file, &given)),
        facts: &facts,
        trusted: false,
        annotations: &lighthouse_plugin::Annotations::default(),
        notices: &lighthouse_plugin::Notices::default(),
        applies: lighthouse_model::ucm::Applicability::default(),
        models: &lighthouse_plugin::NoModels,
        batch: &Default::default(),
        documents: &[],
        queries: &lighthouse_plugin::NoQueries,
    };

    let lists = declarations(&ctx);

    let names: Vec<Vec<&str>> = lists
        .iter()
        .map(|l| l.iter().map(|s| s.name.as_str()).collect())
        .collect();
    assert_eq!(names, [["Store", "Open", "Get"]]);
}

#[test]
fn declarations_in_a_test_file_are_its_fixtures_in_source_order() {
    let mut late = symbol("late", SymbolKind::Function, None, 9);
    late.role = Some(SymbolRole::Fixture);
    let mut early = symbol("early", SymbolKind::Function, None, 2);
    early.role = Some(SymbolRole::Fixture);
    let case = symbol("TestLate", SymbolKind::Function, None, 5);
    let test_file = File {
        test: true,
        ..file()
    };
    let (symbols, places) = placed(&[(&late, 9), (&case, 5), (&early, 2)]);
    let project = Project::merge([Fragment {
        files: vec![test_file.clone()],
        symbols,
        places,
        ..Fragment::default()
    }]);

    let lists = declarations_in(&project, &test_file);

    let names: Vec<Vec<&str>> = lists
        .iter()
        .map(|l| l.iter().map(|s| s.name.as_str()).collect())
        .collect();
    assert_eq!(names, [["early", "late"]]);
}

#[test]
fn exposed_is_everything_that_is_not_private() {
    let mut s = symbol("a", SymbolKind::Function, None, 1);
    assert!(exposed(&s));
    s.visibility = Visibility::Internal;
    assert!(exposed(&s));
    s.visibility = Visibility::Private;
    assert!(!exposed(&s));
}

#[test]
fn owner_key_names_the_owner_or_the_written_path_of_a_method() {
    let ty = symbol("Store", SymbolKind::Type, None, 1);
    let method = symbol("Get", SymbolKind::Method, Some(&ty), 2);
    assert_eq!(owner_key(&method).as_deref(), Some(ty.id.as_str()));
    let mut elsewhere = symbol("Get", SymbolKind::Method, None, 2);
    elsewhere.id = SymbolId::parse("m::Other::Get#method").unwrap();
    assert_eq!(owner_key(&elsewhere).as_deref(), Some("m::Other"));
    assert_eq!(owner_key(&ty), None);
}

#[test]
fn has_word_prefix_wants_a_whole_word() {
    assert!(has_word_prefix("New", "New"));
    assert!(has_word_prefix("NewStore", "New"));
    assert!(has_word_prefix("new_in", "new"));
    assert!(!has_word_prefix("Newton", "New"));
    assert!(!has_word_prefix("newline", "new"));
}

#[test]
fn is_declaration_leaves_out_fields_and_members_of_interfaces() {
    let ty = symbol("Store", SymbolKind::Type, None, 1);
    let iface = symbol("Reader", SymbolKind::Interface, None, 2);
    let field = symbol("id", SymbolKind::Field, Some(&ty), 3);
    let method = symbol("Get", SymbolKind::Method, Some(&ty), 4);
    let declared = symbol("Read", SymbolKind::Method, Some(&iface), 5);
    let (symbols, places) = placed(&[
        (&ty, 1),
        (&iface, 2),
        (&field, 3),
        (&method, 4),
        (&declared, 5),
    ]);
    let project = Project::merge([Fragment {
        files: vec![file()],
        symbols,
        places,
        ..Fragment::default()
    }]);
    assert!(is_declaration(&project, &ty));
    assert!(is_declaration(&project, &method));
    assert!(!is_declaration(&project, &field));
    assert!(!is_declaration(&project, &declared));
}

#[test]
fn the_body_of_a_module_is_no_declaration() {
    let body = symbol(MODULE_BODY, SymbolKind::Function, None, 1);
    let (symbols, places) = placed(&[(&body, 1)]);
    let project = Project::merge([Fragment {
        files: vec![file()],
        symbols,
        places,
        ..Fragment::default()
    }]);
    assert!(!is_declaration(&project, &body));
}
