use lighthouse_engine::{ExampleRun, ModelOptions, RuleTester};
use lighthouse_model::{
    Diagnostic, Fingerprint, Options, Position, Safety, Severity, Span,
    ucm::{Capability, EditOp, FixOutcome, Fragment},
};
use lighthouse_model::{RunScope, ucm::Applicability};
use lighthouse_plugin::{
    Context, Error, FixRequest, Fixer, FixerManifest, Indexed, LanguageManifest, LanguageProvider,
    Plugin, PluginManifest, Registry, Rule, RuleManifest, Source, Workspace,
};
use lighthouse_spec::Catalog;

struct Notes(LanguageManifest);

impl LanguageProvider for Notes {
    fn manifest(&self) -> &LanguageManifest {
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
    word: &'static str,
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

    fn check(&self, ctx: &Context, options: &Options) -> Result<Vec<Diagnostic>, Error> {
        let (file, text) = ctx
            .file
            .ok_or_else(|| Error::Failed("no file".to_owned()))?;
        let word = options
            .get("word")
            .and_then(|w| w.as_str())
            .unwrap_or(self.word);
        let found = text.lines().enumerate().filter(|(_, l)| l.contains(word));
        Ok(found
            .map(|(i, _)| {
                let at = Position {
                    line: u32::try_from(i + 1).unwrap(),
                    col: 1,
                };
                Diagnostic::new(
                    &self.meta.id,
                    self.meta.severity,
                    format!("found {word}"),
                    &file.path,
                    Span { start: at, end: at },
                    Fingerprint::new(&self.meta.id, &file.path.to_string_lossy(), ""),
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
                // A line that says so is fixed into another decision's word.
                text: text.replace(
                    "TODO",
                    if text.contains("(wip)") {
                        "WIP"
                    } else {
                        "DONE"
                    },
                ),
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
        vec![Box::new(Notes(LanguageManifest {
            capabilities: vec![Capability::Overlays],
            ..LanguageManifest::new("notes", vec!["**/*.txt".to_owned()])
        }))]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![Box::new(Done(FixerManifest {
            id: "fake/marker".to_owned(),
            requires: Vec::new(),
        }))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        let marker = |id: &str, word| -> Box<dyn Rule> {
            Box::new(Marker {
                meta: RuleManifest {
                    id: id.to_owned(),
                    uid: None,
                    severity: Severity::Warn,
                    scope: RunScope::File,
                    description: String::new(),
                    docs: String::new(),
                    analyzers: Vec::new(),
                    capabilities: Vec::new(),
                    applicability: Applicability::default(),
                    caching: None,
                },
                word,
            })
        };
        vec![marker("fake/marker", "TODO"), marker("fake/wip", "WIP")]
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
spec:
  title: Marker
  context: Words are flagged.
  scope: {{ subject: file }}
  requirement: A file MUST NOT contain the marker word.
  severity: warn
  options:
    properties:
      word:
        type: string
        default: TODO
        description: Word to flag.
    additionalProperties: false
  check:
    type: cel
    select: file
    where: 'true'
    message: m
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
        &lighthouse_test_support::cel(),
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
    let failures =
        RuleTester::new(registry, &catalog, lighthouse_test_support::bundled()).check_all();
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
    let failures =
        RuleTester::new(registry, &catalog, lighthouse_test_support::bundled()).check_all();
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
    let failures = RuleTester::new(
        Registry::default,
        &catalog,
        lighthouse_test_support::bundled(),
    )
    .check_all();
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
        RuleTester::new(registry, &catalog, lighthouse_test_support::bundled())
            .check(decision)
            .is_empty()
    );
    let failures = RuleTester::new(registry, &catalog, lighthouse_test_support::bundled())
        .language("go")
        .check(decision);
    assert_eq!(failures, ["fake/marker: no example for language `go`"]);
}

const MISSING_TOOL: &str = "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: fake/tool
spec:
  title: Tool
  context: A program judges the file.
  scope: { subject: file }
  requirement: A file MUST satisfy the tool.
  severity: warn
  check:
    type: command
    files: [\"**/*.txt\"]
  languages:
    notes:
      check:
        command: [lighthouse-no-such-tool, \"{file}\"]
        output: status
  examples:
    - name: bad
      language: notes
      kind: invalid
      files: [{ path: a.txt, body: bad }]
      expect: [{ line: 1 }]
    - name: good
      language: notes
      kind: valid
      files: [{ path: a.txt, body: good }]
";

#[test]
fn rule_tester_skipped() {
    let pack = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: fake\nspec:\n  title: Fake\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [tool]\n";
    let catalog = Catalog::from_files(
        [("fake/pack.yaml", pack), ("fake/s/tool.yaml", MISSING_TOOL)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
    let tester = RuleTester::new(registry, &catalog, lighthouse_test_support::bundled());

    let failures = tester.check_all();
    let skipped = tester.skipped(catalog.decision("fake/tool").unwrap());

    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(
        skipped,
        [
            "fake/tool bad: `lighthouse-no-such-tool` is not installed",
            "fake/tool good: `lighthouse-no-such-tool` is not installed"
        ]
    );
}

#[test]
fn a_decision_has_nothing_to_show_in_a_language_it_does_not_apply_to() {
    let catalog = catalog_with("languages:\n  notes: {}\n", HOLDING);
    let decision = catalog.decision("fake/marker").unwrap();
    let tester = || RuleTester::new(registry, &catalog, lighthouse_test_support::bundled());

    assert!(tester().language("go").check(decision).is_empty());
    assert!(!tester().language("go").applies(decision));
    assert!(tester().language("notes").applies(decision));
    assert!(tester().language("notes").check(decision).is_empty());
}

const FIX: &str = "fix:
  safety: unsafe
  type: cel
  edits: '[remove(finding.span)]'
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

    let failures =
        RuleTester::new(registry, &catalog, lighthouse_test_support::bundled()).check_all();

    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn a_fix_example_fails_when_the_fixed_text_differs() {
    let catalog = fixing("          fine\n          TODO later", "          fine");

    let failures =
        RuleTester::new(registry, &catalog, lighthouse_test_support::bundled()).check_all();

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

    let failures = RuleTester::new(
        Registry::default,
        &catalog,
        lighthouse_test_support::bundled(),
    )
    .check_all();

    assert!(
        failures
            .iter()
            .any(|f| f.contains("fix applied nothing") || f.contains("unknown rule")),
        "{failures:?}"
    );
}

#[test]
fn example_run_checks_an_example_with_the_given_configuration() {
    let catalog = catalog(HOLDING);
    let decision = catalog.decisions().next().unwrap();
    let config = lighthouse_spec::Config::parse_inline(
        "plugins = [\"fake\"]\n[rules.\"fake/marker\"]\nlevel = \"warn\"\n",
    )
    .unwrap();
    let run = ExampleRun {
        registry: registry(),
        config,
        catalog: &catalog,
        trusted: false,
        models: ModelOptions::default(),
    };

    let outcome = run.run("fake/marker", &decision.examples[0]).unwrap();

    assert_eq!(outcome.diagnostics.len(), 1);
    assert_eq!(outcome.diagnostics[0].span.start.line, 2);
}

/// Two decisions that flag a word: `fake/marker` (`TODO`) with the examples
/// `marker`, and `fake/wip` (`WIP`) with the examples `wip`.
fn pair(marker: &str, wip: &str, fix: &str) -> Catalog {
    let decision = |name: &str, word: &str, fix: &str, examples: &str| {
        format!(
            "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: fake/{name}
spec:
  title: {name}
  context: Words are flagged.
  scope: {{ subject: file }}
  requirement: A file MUST NOT contain the word.
  severity: warn
  options:
    properties:
      word:
        type: string
        default: {word}
        description: Word to flag.
    additionalProperties: false
  check:
    type: cel
    select: file
    where: 'true'
    message: m
{}  examples:
{}",
            under_spec(fix),
            under_spec(examples)
        )
    };
    let pack = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: fake\nspec:\n  title: Fake\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [marker, wip]\n";
    let marker = decision("marker", "TODO", fix, marker);
    let wip = decision("wip", "WIP", "", wip);
    Catalog::from_files(
        [
            ("fake/pack.yaml", pack),
            ("fake/s/marker.yaml", marker.as_str()),
            ("fake/s/wip.yaml", wip.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect(),
        &lighthouse_test_support::cel(),
    )
    .unwrap()
}

const WIP_CLEAN: &str = "  - name: clean
    language: notes
    kind: valid
    files:
      - path: a.txt
        body: fine
";

const WIP_INVALID: &str = "  - name: flagged
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: WIP
    expect:
      - line: 1
";

fn marker_valid(body: &str, conflicts: &str) -> String {
    format!(
        "  - name: clean
    language: notes
    kind: valid
{conflicts}    files:
      - path: a.txt
        body: |-
{}
",
        body.lines()
            .map(|l| format!("          {l}\n"))
            .collect::<String>()
            .trim_end()
    ) + WIP_INVALID_FOR_MARKER
}

const WIP_INVALID_FOR_MARKER: &str = "  - name: flagged
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: TODO
    expect:
      - line: 1
";

fn failures_of(catalog: &Catalog) -> Vec<String> {
    RuleTester::new(registry, catalog, lighthouse_test_support::bundled()).check_all()
}

#[test]
fn a_valid_example_that_breaks_another_decision_is_a_failure() {
    let catalog = pair(
        &marker_valid("fine\nWIP here", ""),
        &(WIP_CLEAN.to_owned() + WIP_INVALID),
        "",
    );

    let failures = failures_of(&catalog);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].starts_with("fake/marker clean: it also breaks fake/wip"),
        "{failures:?}"
    );
}

#[test]
fn a_declared_conflict_is_reported_and_not_a_failure() {
    let catalog = pair(
        &marker_valid("fine\nWIP here", "    conflicts: [fake/wip]\n"),
        &(WIP_CLEAN.to_owned() + WIP_INVALID),
        "",
    );
    let tester = RuleTester::new(registry, &catalog, lighthouse_test_support::bundled());

    let tested = tester.tested(catalog.decision("fake/marker").unwrap());

    assert!(tested.failures.is_empty(), "{:?}", tested.failures);
    assert_eq!(tested.conflicts.len(), 1, "{:?}", tested.conflicts);
    assert!(tested.conflicts[0].starts_with("fake/marker clean: it also breaks fake/wip"));
}

#[test]
fn a_declared_conflict_that_no_longer_fires_is_a_failure() {
    let catalog = pair(
        &marker_valid("fine", "    conflicts: [fake/wip]\n"),
        &(WIP_CLEAN.to_owned() + WIP_INVALID),
        "",
    );

    let failures = failures_of(&catalog);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("declares a conflict with fake/wip"),
        "{failures:?}"
    );
}

fn fixing_pair(before: &str, after: &str) -> Catalog {
    let (before, after) = (
        before.replace('\n', "\n          "),
        after.replace('\n', "\n          "),
    );
    let fix_example = format!(
        "  - name: todo
    language: notes
    kind: invalid
    files:
      - path: a.txt
        body: |-
          {before}
    expect:
      - line: 1
    fixed:
      - path: a.txt
        body: |-
          {after}
  - name: clean
    language: notes
    kind: valid
    files:
      - path: a.txt
        body: fine
"
    );
    pair(&fix_example, &(WIP_CLEAN.to_owned() + WIP_INVALID), FIX)
}

#[test]
fn a_fix_that_introduces_a_finding_of_another_decision_is_a_failure() {
    let catalog = fixing_pair("TODO (wip)", "WIP (wip)");

    let failures = failures_of(&catalog);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("todo: its fix introduces fake/wip"),
        "{failures:?}"
    );
}

#[test]
fn a_finding_the_example_had_before_its_fix_is_not_introduced_by_it() {
    let catalog = fixing_pair("TODO\nWIP", "DONE\nWIP");

    let failures = failures_of(&catalog);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("todo: it also breaks fake/wip"),
        "{failures:?}"
    );
}
