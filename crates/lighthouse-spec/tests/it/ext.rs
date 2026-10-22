//! The extension libraries the `cel` crate lacks, against their upstream
//! documentation: cel-go's `ext.Regex()` (the README examples of its `regex`
//! section) and the list functions of the Kubernetes CEL library.

use cel::Value;
use lighthouse_spec::{compile, context};

fn eval(source: &str) -> Result<Value, String> {
    compile(source)
        .unwrap()
        .execute(&context().unwrap())
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
fn program_identifiers() {
    let program = compile("regex.extract(a.b, c) + size(d)").unwrap();
    let names = lighthouse_spec::program_identifiers(&program);
    for name in ["a", "c", "d", "size", "regex"] {
        assert!(names.contains(name), "{name}: {names:?}");
    }
}

#[test]
fn free_variables() {
    let free = |source: &str| {
        let program = compile(source).unwrap();
        lighthouse_spec::free_variables(&program)
            .into_iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(free("x.exists(e, e > y)"), ["x", "y"]);
    assert_eq!(free("cel.bind(a, b, a + c)"), ["b", "c"]);
    assert_eq!(free("glob.matches(m, g)"), ["g", "m"]);
}

#[test]
fn matches_does_not_backtrack_on_a_run_of_double_stars() {
    let path = ["a"; 40].join("/") + "/z";
    let pattern = ["**"; 30].join("/") + "/b";
    let started = std::time::Instant::now();
    is(&format!("glob.matches('{path}', '{pattern}')"), "false");
    assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
}

#[test]
fn program_calls() {
    let program = compile("regex.extract(a, b) + size(c) + c.startsWith('x')").unwrap();

    let calls: Vec<(String, Option<String>, bool, usize)> =
        lighthouse_spec::program_calls(&program)
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
