use lighthouse_engine::RuleTester;
use lighthouse_model::{
    Capability, Diagnostic, EditOp, Fingerprint, FixOutcome, Fragment, Options, Position, Safety,
    Severity, Span,
};
use lighthouse_plugin::{
    Ctx, Error, FixRequest, Fixer, FixerManifest, Indexed, LanguageProvider, Plugin,
    PluginManifest, ProviderManifest, Registry, Rule, RuleManifest, Scope, Source, Workspace,
};
use lighthouse_spec::Catalog;

struct Notes(ProviderManifest);

impl LanguageProvider for Notes {
    fn manifest(&self) -> &ProviderManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        Ok(Indexed {
            fragments: files
                .iter()
                .map(|s| Fragment {
                    files: vec![s.file.clone()],
                    ..Fragment::default()
                })
                .collect(),
            ..Indexed::default()
        })
    }
}

/// Flags every line containing the configured word (default `TODO`).
struct Marker {
    meta: RuleManifest,
}

impl Rule for Marker {
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }

    fn validate(&self, options: &Options) -> Result<(), Error> {
        match options.keys().find(|k| *k != "word") {
            Some(key) => Err(Error::Options {
                rule: self.meta.id.clone(),
                message: format!("unknown option `{key}`"),
            }),
            None => Ok(()),
        }
    }

    fn check(&self, ctx: &Ctx, options: &Options) -> Result<Vec<Diagnostic>, Error> {
        let (file, text) = ctx
            .file
            .ok_or_else(|| Error::Failed("no file".to_owned()))?;
        let word = options
            .get("word")
            .and_then(|w| w.as_str())
            .unwrap_or("TODO");
        let found = text.lines().enumerate().filter(|(_, l)| l.contains(word));
        Ok(found
            .map(|(i, _)| {
                let at = Position {
                    line: u32::try_from(i + 1).unwrap(),
                    col: 1,
                };
                Diagnostic::new(
                    "fake/marker",
                    self.meta.severity,
                    format!("found {word}"),
                    &file.path,
                    Span { start: at, end: at },
                    Fingerprint::of("fake/marker", &file.path.to_string_lossy(), ""),
                )
            })
            .collect())
    }
}

/// Rewrites the flagged line, `TODO` into `DONE`.
struct Done(FixerManifest);

impl Fixer for Done {
    fn manifest(&self) -> &FixerManifest {
        &self.0
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, Error> {
        let line = request.finding.span.start.line;
        let text = request
            .text
            .lines()
            .nth(line as usize - 1)
            .unwrap_or_default();
        let at = |col: usize| Position {
            line,
            col: u32::try_from(col + 1).unwrap(),
        };
        Ok(FixOutcome::Proposed {
            description: "done".to_owned(),
            ops: vec![EditOp::Replace {
                file: request.finding.file.clone(),
                span: Span {
                    start: at(0),
                    end: at(text.len()),
                },
                text: text.replace("TODO", "DONE"),
            }],
            safety: Safety::Safe,
        })
    }
}

struct Fake(PluginManifest);

impl Plugin for Fake {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Notes(ProviderManifest {
            capabilities: vec![Capability::Overlays],
            ..ProviderManifest::new("notes", vec!["**/*.txt".to_owned()])
        }))]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![Box::new(Done(FixerManifest {
            id: "fake/marker".to_owned(),
            requires: Vec::new(),
        }))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![Box::new(Marker {
            meta: RuleManifest {
                id: "fake/marker".to_owned(),
                severity: Severity::Warn,
                scope: Scope::File,
                description: String::new(),
                docs: String::new(),
                analyzers: Vec::new(),
                capabilities: Vec::new(),
                citation: None,
                strict: false,
            },
        })]
    }
}

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry
        .register(&Fake(PluginManifest {
            id: "fake".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    registry
}

fn catalog(examples: &str) -> Catalog {
    catalog_with("", examples)
}

/// `text` one level deeper, as it sits under `spec:`.
fn under_spec(text: &str) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}

fn catalog_with(fix: &str, examples: &str) -> Catalog {
    let decision = format!(
        "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: fake/marker
  labels:
    lighthouse/pack: fake
    lighthouse/section: s
spec:
  title: Marker
  intent: Words are flagged.
  scope: {{ subject: file }}
  requirement: A file MUST NOT contain the marker word.
  enforcement: heuristic
  evidence: [word]
  options:
    type: object
    properties:
      word:
        type: string
        default: TODO
        description: Word to flag.
    additionalProperties: false
  check:
    type: builtin
    id: fake/marker
{}  examples:
{}",
        under_spec(fix),
        under_spec(examples)
    );
    let pack = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: fake\nspec:\n  title: Fake\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [marker]\n";
    Catalog::from_files(
        [
            ("fake/pack.yaml", pack),
            ("fake/s/marker.yaml", decision.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect(),
    )
    .unwrap()
}

const HOLDING: &str = "  - name: flagged
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: |-
          fine
          TODO later
    expect:
      - line: 2
        message: found TODO
  - name: clean
    language: notes
    kind: valid
    files:
      - path: a.txt
        body: fine
  - name: custom-word
    language: notes
    kind: invalid
    options:
      word: FIXME
    files:
      - path: docs/notes.txt
        body: |-
          TODO is fine here
          FIXME is not
      - path: other.txt
        body: FIXME too
    expect:
      - line: 2
      - line: 1
";

#[test]
fn holding_examples_pass_through_the_engine() {
    let catalog = catalog(HOLDING);
    let failures = RuleTester::new(registry, &catalog).check_all();
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn broken_examples_report_the_decision_example_and_reason() {
    let catalog = catalog(
        "  - name: wrong-line
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: |-
          fine
          TODO later
    expect:
      - line: 1
  - name: unexpected
    language: notes
    kind: valid
    files:
      - path: a.txt
        body: TODO
  - name: wrong-message
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: TODO
    expect:
      - line: 1
        message: nope
  - name: silent
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: clean
    expect:
      - line: 1
",
    );
    let failures = RuleTester::new(registry, &catalog).check_all();
    assert_eq!(failures.len(), 4, "{failures:?}");
    assert!(failures[0].starts_with(
        "fake/marker wrong-line: expected diagnostics at lines [1], got [2: found TODO]"
    ));
    assert!(
        failures[1]
            .starts_with("fake/marker unexpected: expected no diagnostics, got [1: found TODO]")
    );
    assert!(failures[2].contains("wrong-message: line 1: `found TODO` lacks `nope`"));
    assert!(failures[3].contains("silent: expected diagnostics at lines [1], got none"));
}

#[test]
fn engine_errors_in_an_example_are_failures_not_panics() {
    let catalog = catalog(HOLDING);
    let failures = RuleTester::new(Registry::default, &catalog).check_all();
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(
        failures[0].starts_with("fake/marker flagged: "),
        "{failures:?}"
    );
}

#[test]
fn rule_tester_check() {
    let catalog = catalog(HOLDING);
    let decision = catalog.decision("fake/marker").unwrap();
    assert!(
        RuleTester::new(registry, &catalog)
            .check(decision)
            .is_empty()
    );
    let failures = RuleTester::new(registry, &catalog)
        .language("go")
        .check(decision);
    assert_eq!(failures, ["fake/marker: no example for language `go`"]);
}

const FIX: &str = "fix:
  safety: suggested
  type: ops
  ops:
    - op: delete
      file: finding.file
      span: finding.span
";

fn fixing(fixed: &str, again: &str) -> Catalog {
    catalog_with(
        FIX,
        &format!(
            "  - name: todo
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: |-
          fine
          TODO later
    expect:
      - line: 2
    fixed:
      - path: a.txt
        body: |-
{fixed}
  - name: ok
    language: notes
    kind: valid
    files:
      - path: a.txt
        body: |-
{again}
"
        ),
    )
}

#[test]
fn a_fix_example_passes_when_the_fix_gives_its_fixed_text_and_the_rule_stops_firing() {
    let catalog = fixing("          fine\n          DONE later", "          fine");

    let failures = RuleTester::new(registry, &catalog).check_all();

    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn a_fix_example_fails_when_the_fixed_text_differs() {
    let catalog = fixing("          fine\n          TODO later", "          fine");

    let failures = RuleTester::new(registry, &catalog).check_all();

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("todo: fix of `a.txt` is not what `fixed` says"),
        "{failures:?}"
    );
    assert!(failures[0].contains("+DONE later"));
}

#[test]
fn a_fix_example_fails_when_the_fixer_applies_nothing() {
    let catalog = fixing("          fine\n          DONE later", "          fine");

    let failures = RuleTester::new(Registry::default, &catalog).check_all();

    assert!(
        failures
            .iter()
            .any(|f| f.contains("fix applied nothing") || f.contains("unknown rule")),
        "{failures:?}"
    );
}
