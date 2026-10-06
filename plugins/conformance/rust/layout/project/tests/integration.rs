use layout::add;

mod common;

struct Case {
    a: i32,
    b: i32,
}

#[test]
fn through_the_public_api() {
    assert_eq!(add(1, 1), common::two());
}

#[test]
fn table_of_structs() {
    let cases = vec![Case { a: 1, b: 1 }, Case { a: 2, b: 0 }];
    for case in &cases {
        assert_eq!(add(case.a, case.b), 2);
    }
}

#[tokio::test]
async fn async_case() {
    add(1, 1);
}

#[rstest]
#[case(1)]
#[case(2)]
fn cases_by_attribute(#[case] n: i32) {
    add(n, 0);
}

#[test_case::test_case(1, 2)]
#[test_case::test_case(2, 3)]
fn test_case_by_path(a: i32, b: i32) {
    add(a, b);
}

#[rstest]
#[case(1)]
fn one_case_is_a_scenario(#[case] n: i32) {
    add(n, 0);
}

#[other::test]
fn not_a_test_by_path() {
    add(0, 0);
}
