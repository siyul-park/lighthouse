//! The one CEL environment: the standard library with the extensions the
//! `cel` crate ships, and the official ones it lacks (see `ext`).

use cel::Value;
use lighthouse_spec::{Env, children};

fn cel() -> Env {
    lighthouse_test_support::cel()
}

fn eval(source: &str) -> Value {
    let cel = cel();
    cel.compile(source)
        .unwrap()
        .execute(&cel.context())
        .unwrap()
}

fn list(items: &[&str]) -> Value {
    let items: Vec<Value> = items.iter().map(|item| Value::from(*item)).collect();
    Value::List(items.into())
}

#[test]
fn compile_reads_the_optional_syntax_and_refuses_a_broken_expression() {
    let cel = cel();
    assert!(cel.compile("line.?name.orValue(\"\")").is_ok());
    assert!(cel.compile("line.name ==").is_err());
}

#[test]
fn environment_has_the_strings_lists_math_and_encoders_extensions() {
    assert_eq!(eval("\"a,b\".split(\",\")"), list(&["a", "b"]));
    assert_eq!(eval("\"ab\".split(\"\")"), list(&["a", "b"]));
    assert_eq!(eval("\"x\".replace(\"x\", \"y\", 1)"), Value::from("y"));
    assert_eq!(eval("\" a \".trim()"), Value::from("a"));
    assert_eq!(eval("math.greatest(1, 2)"), Value::from(2_i64));
    assert_eq!(eval("[1, 2, 3].slice(1, 3).size()"), Value::from(2_i64));
    assert_eq!(eval("base64.encode(b\"hi\")"), Value::from("aGk="));
}

#[test]
fn environment_is_the_same_one_for_every_clone() {
    let cel = cel();
    assert!(std::sync::Arc::ptr_eq(
        &cel.environment(),
        &cel.clone().environment()
    ));
}

#[test]
fn two_environments_do_not_share_their_libraries() {
    let standard = Env::standard().unwrap();

    assert!(
        standard.compile("regex.extract('a', 'a')").is_ok(),
        "parses"
    );
    let call = standard
        .compile("regex.extract('a', 'a')")
        .unwrap()
        .execute(&standard.context());
    assert!(
        call.is_err(),
        "a library is not in the standard environment"
    );
    assert!(!standard.is_namespace("regex"));
    assert!(cel().is_namespace("regex"));
}

#[test]
fn context_evaluates_with_the_variables_added_to_it() {
    let cel = cel();
    let mut context = cel.context();
    context.add_variable_from_value("name", "lighthouse");

    let upper = cel.compile("name.upperAscii()").unwrap().execute(&context);

    assert_eq!(upper.unwrap(), Value::from("LIGHTHOUSE"));
}

/// A list of `n` integers as a variable of the context.
fn evaluate_over(n: i64, source: &str) -> Result<Value, cel::ExecutionError> {
    let cel = cel();
    let mut context = cel.context();
    let items: Vec<Value> = (0..n).map(Value::Int).collect();
    context.add_variable_from_value("xs", Value::List(items.into()));
    cel.compile(source).unwrap().execute(&context)
}

#[test]
fn map_over_a_list_of_thousands_of_elements_gives_every_element() {
    for n in [100, 1_000, 4_000, 6_000, 30_000] {
        for source in ["xs.map(x, x).size()", "xs.map(x, {\"p\": x}).size()"] {
            let size = evaluate_over(n, source);

            assert_eq!(size, Ok(Value::Int(n)), "{n} elements, {source}");
        }
    }
}

#[test]
fn the_children_of_an_expression_are_its_operands() {
    let program = cel().compile("f(a, [b, c]).d").unwrap();
    let select = program.expression();
    assert_eq!(children(select).len(), 1, "a selection has its operand");
    let call = children(select)[0];
    assert_eq!(children(call).len(), 2, "a call has its arguments");
    assert_eq!(
        children(children(call)[1]).len(),
        2,
        "a list has its elements"
    );
    assert!(
        children(children(call)[0]).is_empty(),
        "an identifier has none"
    );
}

#[test]
fn compile_shared() {
    let cel = cel();
    let first = cel.compile_shared("1 + 1").unwrap();
    let again = cel.clone().compile_shared("1 + 1").unwrap();

    assert!(
        std::sync::Arc::ptr_eq(&first, &again),
        "one program per source"
    );
    assert!(cel.compile_shared("1 +").is_err());
}

#[test]
fn program_identifiers() {
    let program = cel().compile("regex.extract(a.b, c) + size(d)").unwrap();
    let names = lighthouse_spec::program_identifiers(&program);
    for name in ["a", "c", "d", "size", "regex"] {
        assert!(names.contains(name), "{name}: {names:?}");
    }
}

#[test]
fn free_variables() {
    let cel = cel();
    let free = |source: &str| {
        let program = cel.compile(source).unwrap();
        lighthouse_spec::free_variables(&cel, &program)
            .into_iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(free("x.exists(e, e > y)"), ["x", "y"]);
    assert_eq!(free("cel.bind(a, b, a + c)"), ["b", "c"]);
    assert_eq!(free("glob.matches(m, g)"), ["g", "m"]);
}

#[test]
fn program_calls() {
    let cel = cel();
    let program = cel
        .compile("regex.extract(a, b) + size(c) + c.startsWith('x')")
        .unwrap();

    let calls: Vec<(String, Option<String>, bool, usize)> =
        lighthouse_spec::program_calls(&cel, &program)
            .into_iter()
            .filter(|call| call.name.chars().all(|c| c.is_ascii_alphabetic()))
            .map(|call| (call.name, call.namespace, call.member, call.args))
            .collect();

    assert_eq!(
        calls,
        [
            ("extract".to_owned(), Some("regex".to_owned()), false, 2),
            ("size".to_owned(), None, false, 1),
            ("startsWith".to_owned(), None, true, 1),
        ]
    );
}

#[test]
fn new() {
    let cel = Env::new(lighthouse_spec::base().unwrap(), ["mine".to_owned()]);

    assert!(cel.is_namespace("mine"));
    assert!(cel.is_namespace("math"), "the namespaces of the cel crate");
    assert!(!cel.is_namespace("other"));
}
