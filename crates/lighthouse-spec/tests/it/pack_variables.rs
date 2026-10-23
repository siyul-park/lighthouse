//! The `variables` a pack shares with its decisions.

use lighthouse_resource::API_VERSION;
use lighthouse_spec::{Catalog, CheckKind};
use lighthouse_test_support::catalog::{Files, decision, files};

const EXAMPLES: &str = "  examples:
    - name: bad
      language: go
      kind: invalid
      files: [{ path: a.go, body: x }]
      expect: [{ line: 1 }]
    - name: good
      language: go
      kind: valid
      files: [{ path: a.go, body: y }]
";

/// Pack `p` with `variables` (YAML under `spec:`) and decision `p/a` whose
/// check has `own` variables and the condition `condition`.
fn catalog(variables: &str, own: &str, condition: &str) -> Result<Catalog, lighthouse_spec::Error> {
    let pack = format!(
        "apiVersion: {API_VERSION}\nkind: Pack\nmetadata:\n  name: p\nspec:\n  title: P\n  intro: x\n{variables}  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [a]\n"
    );
    let spec = format!(
        "  title: A\n  context: i\n  scope: {{ subject: file }}\n  requirement: A MUST b.\n  severity: error\n  check:\n    type: cel\n    select: file\n{own}    where: \"{condition}\"\n    message: m\n{EXAMPLES}"
    );
    let all: Files = files(&[
        ("p/pack.yaml", pack),
        ("p/s/a.yaml", decision("p/a", &spec)),
    ]);
    Catalog::from_files(all, &lighthouse_test_support::cel())
}

fn imported(catalog: &Catalog) -> Vec<String> {
    match &catalog
        .decision("p/a")
        .unwrap()
        .check
        .as_ref()
        .unwrap()
        .kind
    {
        CheckKind::Cel(cel) => cel.variables.iter().map(|v| v.name.clone()).collect(),
        other => panic!("cel expected, got {}", other.name()),
    }
}

const SHARED: &str = "  variables:
    - name: twice
      expression: size(file.path) * 2
    - name: quad
      expression: twice * 2
    - name: unused
      expression: '1'
    - name: plus
      params: [n]
      expression: n + 1
";

#[test]
fn a_decision_gets_the_pack_variables_it_names_before_its_own() {
    let own = "    variables:\n      - { name: mine, expression: quad + 1 }\n";
    let catalog = catalog(SHARED, own, "mine > 0").unwrap();

    assert_eq!(imported(&catalog), ["twice", "quad", "mine"]);
}

#[test]
fn a_function_of_the_pack_is_imported_by_its_call() {
    let catalog = catalog(SHARED, "", "plus(1) > 1").unwrap();

    assert_eq!(imported(&catalog), ["plus"]);
}

#[test]
fn a_decision_that_names_no_pack_variable_gets_none() {
    let catalog = catalog(SHARED, "", "true").unwrap();

    assert!(imported(&catalog).is_empty());
}

#[test]
fn a_decision_does_not_redefine_a_pack_variable_it_names() {
    let own = "    variables:\n      - { name: twice, expression: '3' }\n";

    let error = catalog(SHARED, own, "twice > 0").unwrap_err().to_string();

    assert!(error.contains("variable of its pack"), "{error}");
}

#[test]
fn a_pack_variable_must_compile_and_be_named_once() {
    let bad = "  variables:\n    - { name: a, expression: '1 +' }\n";
    assert!(catalog(bad, "", "true").is_err());
    let twice =
        "  variables:\n    - { name: a, expression: '1' }\n    - { name: a, expression: '2' }\n";
    let error = catalog(twice, "", "true").unwrap_err().to_string();
    assert!(error.contains("defined twice"), "{error}");
}

#[test]
fn identifiers_name_the_variables_the_comprehensions_and_the_calls_of_an_expression() {
    let names = lighthouse_spec::identifiers(
        &lighthouse_test_support::cel(),
        "xs.filter(x, f(x) > limit)",
    );

    for name in ["f", "limit", "x", "xs"] {
        assert!(names.contains(name), "{name} in {names:?}");
    }
    assert!(lighthouse_spec::identifiers(&lighthouse_test_support::cel(), "1 +").is_empty());
}

#[test]
fn variable() {
    for variable in [
        "{ name: Bad, expression: '1' }",
        "{ name: file, expression: '1' }",
        "{ name: f, params: [options], expression: '1' }",
        "{ name: f, params: [File], expression: '1' }",
    ] {
        let shared = format!("  variables:\n    - {variable}\n");
        assert!(catalog(&shared, "", "true").is_err(), "{variable}");
    }
    let usable = "  variables:\n    - { name: f, params: [a, b_2], expression: 'a + b_2' }\n";
    assert!(catalog(usable, "", "true").is_ok());
}
