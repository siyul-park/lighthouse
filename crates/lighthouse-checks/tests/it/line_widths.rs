//! The `line_widths` fact of a file: widths in characters, and the lines a
//! formatter cannot wrap.

use lighthouse_checks::Checks;
use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_spec::Catalog;

const DECISION: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: demo/wide
spec:
  title: Lines are narrow
  context: A line is at most five characters wide.
  scope: { subject: file }
  requirement: A line MUST NOT be wider than five characters.
  severity: warn
  check:
    type: cel
    select: file
    variables:
      - name: wide
        expression: file.line_widths.filter(l, l.width > 5 && !l.exempt)
    where: size(wide) > 0
    message: "line {{ wide[0].line }} is {{ wide[0].width }} wide"
    at:
      line: wide[0].line
  examples:
    - name: first-wide-line
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: "ééééé\nééééé\n\t\t\t\t\t\nééééééé\nlonger"
      expect:
        - line: 4
    - name: unbreakable-lines
      language: text
      kind: valid
      files:
        - path: a.txt
          body: "// https://example.com/a/long/path\n\"a long literal\",\nimport a.b.c.d\nuse a::b::c;\n/* http://x.io/aaa */"
    - name: a-url-with-words-is-wide
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: "// see https://example.com/a/long/path"
      expect:
        - line: 1
"#;

const PACK: &str = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: demo\nspec:\n  title: Demo\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [wide]\n";

fn catalog() -> Catalog {
    let files = [("demo/pack.yaml", PACK), ("demo/s/wide.yaml", DECISION)]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    Catalog::from_files(files, &lighthouse_test_support::cel()).unwrap()
}

fn registry() -> Registry {
    let mut registry = lighthouse_test_support::registry();
    registry
        .register(
            &Checks::new(&lighthouse_test_support::composition(), "demo", &catalog()).unwrap(),
        )
        .unwrap();
    registry
}

#[test]
fn line_widths_count_characters_and_skip_what_cannot_wrap() {
    let failures =
        RuleTester::new(registry, &catalog(), lighthouse_test_support::bundled()).check_all();
    assert!(failures.is_empty(), "{failures:?}");
}
