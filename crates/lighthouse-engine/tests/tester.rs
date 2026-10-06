use lighthouse_engine::RuleTester;
use lighthouse_model::{
    Capability, Diagnostic, File, Fingerprint, Fragment, Options, Position, Severity, Span,
};
use lighthouse_plugin::{
    Conventions, Ctx, Error, LanguageProvider, Manifest, Plugin, Registry, Rule, RuleMeta, Scope,
    Workspace,
};
use lighthouse_spec::Catalog;

struct Notes {
    globs: Vec<String>,
}

impl LanguageProvider for Notes {
    fn id(&self) -> &str {
        "notes"
    }
    fn globs(&self) -> &[String] {
        &self.globs
    }
    fn conventions(&self) -> Conventions {
        Conventions::default()
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
    fn index(&self, _: &Workspace, file: &File, _: &str) -> Result<Fragment, Error> {
        Ok(Fragment {
            files: vec![file.clone()],
            ..Fragment::default()
        })
    }
}

/// Flags every line containing the configured word (default `TODO`).
struct Marker {
    meta: RuleMeta,
}

impl Rule for Marker {
    fn meta(&self) -> &RuleMeta {
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

struct Fake;

impl Plugin for Fake {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "fake".to_owned(),
            version: "0".to_owned(),
        }
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Notes {
            globs: vec!["**/*.txt".to_owned()],
        })]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![Box::new(Marker {
            meta: RuleMeta {
                id: "fake/marker".to_owned(),
                severity: Severity::Warn,
                scope: Scope::File,
                description: String::new(),
                docs: String::new(),
                analyzers: Vec::new(),
                capabilities: Vec::new(),
                citation: None,
            },
        })]
    }
}

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry.register(&Fake).unwrap();
    registry
}

fn catalog(examples: &str) -> Catalog {
    let pattern = format!(
        "id: fake/marker
title: Marker
intent: Words are flagged.
scope: file
requirement: A file MUST NOT contain the marker word.
enforcement: heuristic
evidence: [word]
options:
  word:
    type: string
    default: TODO
    description: Word to flag.
implementation:
  builtin: fake/marker
examples:
{examples}"
    );
    Catalog::from_files(
        [
            (
                "fake/pack.yaml",
                "id: fake\ntitle: Fake\nintro: x\nsections: [s]\n",
            ),
            (
                "fake/s/section.yaml",
                "id: s\ntitle: S\nintro: x\npatterns: [marker]\n",
            ),
            ("fake/s/marker.yaml", pattern.as_str()),
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
fn broken_examples_report_the_pattern_example_and_reason() {
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
