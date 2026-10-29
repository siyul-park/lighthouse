//! The libraries of the plugin, against their upstream documentation: cel-go's
//! `ext.Regex()` (the README examples of its `regex` section), the list
//! functions of the Kubernetes CEL library, and cel-go's bindings.

use std::sync::{Arc, OnceLock};

use cel::{Context, Env, Value};
use lang_cel::Cel;
use lighthouse_plugin::Registry;

/// The standard environment with every library of the plugin registered.
pub(crate) fn environment() -> Arc<Env> {
    static ENV: OnceLock<Arc<Env>> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut registry = Registry::default();
        registry.register(&Cel::new()).unwrap();
        let mut env = lighthouse_spec::base().unwrap();
        for library in registry.cel().libraries() {
            library.register(&mut env).unwrap();
        }
        Arc::new(env)
    })
    .clone()
}

fn eval(source: &str) -> Result<Value, String> {
    let env = environment();
    env.compile(source)
        .map_err(|e| e.to_string())?
        .execute(&Context::with_env(env))
        .map_err(|e| e.to_string())
}

fn is(source: &str, expected: &str) {
    assert_eq!(eval(source), eval(expected), "{source}");
}

fn fails(source: &str, why: &str) {
    let error = eval(source).unwrap_err();
    assert!(error.contains(why), "{source}: {error}");
}

#[test]
fn regex_replace_replaces_every_match_or_the_first_count_of_them() {
    is(
        "regex.replace('hello world hello', 'hello', 'hi')",
        "'hi world hi'",
    );
    is("regex.replace('banana', 'a', 'x', 0)", "'banana'");
    is("regex.replace('banana', 'a', 'x', 1)", "'bxnana'");
    is("regex.replace('banana', 'a', 'x', 2)", "'bxnxna'");
    is("regex.replace('banana', 'a', 'x', -12)", "'bxnxnx'");
}

#[test]
fn regex_replace_refers_to_groups_as_backslash_digits() {
    is(
        "regex.replace('foo bar', '(fo)o (ba)r', '\\\\2 \\\\1')",
        "'ba fo'",
    );
    is("regex.replace('abc', 'b', '[\\\\0]')", "'a[b]c'");
    is("regex.replace('a.b', '\\\\.', '\\\\\\\\')", "'a\\\\b'");
}

#[test]
fn regex_replace_refuses_a_bad_replacement_or_pattern() {
    fails(
        "regex.replace('test', '(.)', '$2')",
        "invalid replace string",
    );
    fails("regex.replace('test', '(.)', '\\\\2')", "no group 2");
    fails(
        "regex.replace('id=123', 'id=(?P<value>\\\\d+)', 'value: \\\\values')",
        "invalid replace string",
    );
    fails(
        "regex.replace('foo bar', '(', '$2 $1')",
        "invalid regex string",
    );
}

#[test]
fn regex_extract_is_the_first_match_as_an_optional() {
    is(
        "regex.extract('hello world', 'hello(.*)')",
        "optional.of(' world')",
    );
    is(
        "regex.extract('item-A, item-B', 'item-(\\\\w+)')",
        "optional.of('A')",
    );
    is("regex.extract('HELLO', 'hello')", "optional.none()");
    is("regex.extract('abc', 'b')", "optional.of('b')");
    fails(
        "regex.extract('testuser@testdomain', '(.*)@([^.]*)')",
        "multiple capture groups",
    );
}

#[test]
fn regex_extract_all_lists_every_match() {
    is(
        "regex.extractAll('id:123, id:456', 'id:\\\\d+')",
        "['id:123', 'id:456']",
    );
    is("regex.extractAll('id:123, id:456', 'assa')", "[]");
    is("regex.extractAll('a1 b22', '[a-z](\\\\d+)')", "['1', '22']");
    fails(
        "regex.extractAll('testuser@testdomain', '(.*)@([^.]*)')",
        "multiple capture groups",
    );
}

#[test]
fn a_list_knows_the_index_of_an_element() {
    is("['a', 'b', 'a'].indexOf('a')", "0");
    is("['a', 'b', 'a'].lastIndexOf('a')", "2");
    is("['a', 'b'].indexOf('z')", "-1");
    is("['a', 'b'].lastIndexOf('z')", "-1");
    is("[1, 2, 3].indexOf(2.0)", "1");
    is("[].indexOf(1)", "-1");
}

#[test]
fn the_string_indexof_is_still_the_strings_one() {
    is("'tacocat'.indexOf('cat')", "4");
}

#[test]
fn cel_bind_names_a_value_computed_once() {
    is("cel.bind(a, 'hello', a + ', ' + a)", "'hello, hello'");
    is("cel.bind(a, 1, cel.bind(b, a + 1, a + b))", "3");
    is("cel.bind(a, [1, 2], a.map(x, x * 2))", "[2, 4]");
    is("[1, 2].map(x, cel.bind(y, x + 1, y * y))", "[4, 9]");
    is("cel.bind(a, 1, cel.bind(a, 2, a))", "2");
}

#[test]
fn matches() {
    is("glob.matches('app/http', 'app/*')", "true");
    is("glob.matches('app/http/x', 'app/*')", "false");
    is("glob.matches('app', 'app/**')", "true");
    is("glob.matches('app::http::x', 'app/**')", "true");
    is("glob.matches('web/app', 'app/**')", "false");
}

#[test]
fn matches_does_not_backtrack_on_a_run_of_double_stars() {
    let path = ["a"; 40].join("/") + "/z";
    let pattern = ["**"; 30].join("/") + "/b";
    let started = std::time::Instant::now();
    is(&format!("glob.matches('{path}', '{pattern}')"), "false");
    assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
}
