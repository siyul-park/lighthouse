//! The one CEL environment: the standard library with the extensions the
//! `cel` crate ships, and the official ones it lacks (see `ext`).

use cel::Value;
use lighthouse_spec::{children, compile, context, environment};

fn eval(source: &str) -> Value {
    compile(source)
        .unwrap()
        .execute(&context().unwrap())
        .unwrap()
}

fn list(items: &[&str]) -> Value {
    let items: Vec<Value> = items.iter().map(|item| Value::from(*item)).collect();
    Value::List(items.into())
}

#[test]
fn compile_reads_the_optional_syntax_and_refuses_a_broken_expression() {
    assert!(compile("line.?name.orValue(\"\")").is_ok());
    assert!(compile("line.name ==").is_err());
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
fn environment_is_the_same_one_every_time() {
    assert!(std::sync::Arc::ptr_eq(
        &environment().unwrap(),
        &environment().unwrap()
    ));
}

#[test]
fn context_evaluates_with_the_variables_added_to_it() {
    let mut context = context().unwrap();
    context.add_variable_from_value("name", "lighthouse");

    let upper = compile("name.upperAscii()").unwrap().execute(&context);

    assert_eq!(upper.unwrap(), Value::from("LIGHTHOUSE"));
}

/// A list of `n` integers as a variable of the context.
fn evaluate_over(n: i64, source: &str) -> Result<Value, cel::ExecutionError> {
    let mut context = context().unwrap();
    let items: Vec<Value> = (0..n).map(Value::Int).collect();
    context.add_variable_from_value("xs", Value::List(items.into()));
    compile(source).unwrap().execute(&context)
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
    let program = compile("f(a, [b, c]).d").unwrap();
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
    let first = lighthouse_spec::compile_shared("1 + 1").unwrap();
    let again = lighthouse_spec::compile_shared("1 + 1").unwrap();

    assert!(
        std::sync::Arc::ptr_eq(&first, &again),
        "one program per source"
    );
    assert!(lighthouse_spec::compile_shared("1 +").is_err());
}
