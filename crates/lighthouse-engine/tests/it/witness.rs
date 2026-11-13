//! The oracle over toy order decisions: a toy provider claims the rendered
//! Rust files and each toy rule flags a file that puts one kind of
//! declaration below another, read off the text.

use lighthouse_engine::{Basis, Cell, Kind, Oracle, Searched, Verdict, Visibility};
use lighthouse_graph::{Graph, Options};
use lighthouse_model::{
    Diagnostic, Fingerprint, Options as RuleOptions, Position, RunScope, Severity, Span,
    ucm::Applicability,
};
use lighthouse_plugin::{
    Context, Error, Fragments, Indexed, LanguageManifest, LanguageProvider, Plugin, PluginManifest,
    Registry, Rule, RuleManifest, Unit, Workspace,
};
use lighthouse_spec::{Catalog, Decision};
use serde_json::{Map, Value};

/// Claims the Rust files and reads nothing of them; or fails when it must.
struct Provider {
    manifest: LanguageManifest,
    crash: bool,
}

impl LanguageProvider for Provider {
    fn manifest(&self) -> &LanguageManifest {
        &self.manifest
    }

    fn index(&self, _: &Workspace, units: &[Unit], _: &[String]) -> Result<Indexed, Error> {
        if self.crash {
            return Err(Error::Failed("the provider crashed".to_owned()));
        }
        Ok(Indexed {
            units: units
                .iter()
                .map(|unit| {
                    Fragments::new(
                        unit.id,
                        unit.files
                            .iter()
                            .map(|s| lighthouse_model::ucm::Fragment {
                                files: vec![s.file.clone()],
                                ..Default::default()
                            })
                            .collect(),
                    )
                })
                .collect(),
            ..Indexed::default()
        })
    }
}

/// How a toy rule judges the text of a file.
#[derive(Clone, Copy)]
enum Judge {
    /// Flags a file where a line that starts with `second` comes before one
    /// that starts with `first`: `first` must be declared above `second`.
    Above(&'static str, &'static str),
    /// Flags every package of the scratch project.
    Always,
    Never,
    /// Flags the file that is no package of it.
    Stray,
}

struct Toy {
    manifest: RuleManifest,
    judge: Judge,
}

impl Rule for Toy {
    fn manifest(&self) -> &RuleManifest {
        &self.manifest
    }

    fn validate(&self, _: &RuleOptions) -> Result<(), Error> {
        Ok(())
    }

    fn check(&self, ctx: &Context, _: &RuleOptions) -> Result<Vec<Diagnostic>, Error> {
        let (file, text) = ctx
            .file
            .ok_or_else(|| Error::Failed("no file".to_owned()))?;
        let at = |prefix: &str| text.get().lines().position(|line| line.starts_with(prefix));
        let flagged = match self.judge {
            Judge::Always => file.path.to_string_lossy().contains("/p"),
            Judge::Stray => file.path.ends_with("lib.rs"),
            Judge::Never => false,
            Judge::Above(first, second) => {
                matches!((at(first), at(second)), (Some(a), Some(b)) if b < a)
            }
        };
        let position = Position { line: 1, col: 1 };
        Ok(flagged
            .then(|| {
                Diagnostic::new(
                    &self.manifest.id,
                    self.manifest.severity,
                    "out of order".to_owned(),
                    &file.path,
                    Span {
                        start: position,
                        end: position,
                    },
                    Fingerprint::new(&self.manifest.id, &file.path.to_string_lossy(), ""),
                )
            })
            .into_iter()
            .collect())
    }
}

struct Plug {
    manifest: PluginManifest,
    crash: bool,
}

/// The toy decisions: id, and how it judges.
const RULES: [(&str, Judge); 7] = [
    ("toy/pub-then-priv", Judge::Above("pub fn ", "fn ")),
    ("toy/priv-then-pub", Judge::Above("fn ", "pub fn ")),
    ("toy/type-then-pub", Judge::Above("pub struct ", "pub fn ")),
    ("toy/priv-then-type", Judge::Above("fn ", "pub struct ")),
    ("toy/never", Judge::Never),
    ("toy/always", Judge::Always),
    ("toy/stray", Judge::Stray),
];

impl Plugin for Plug {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Provider {
            manifest: LanguageManifest::new("rust", vec!["**/*.rs".to_owned()]),
            crash: self.crash,
        })]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        RULES
            .iter()
            .map(|(id, judge)| -> Box<dyn Rule> {
                Box::new(Toy {
                    manifest: RuleManifest {
                        id: (*id).to_owned(),
                        uid: None,
                        severity: Severity::Error,
                        scope: RunScope::File,
                        description: String::new(),
                        docs: String::new(),
                        analyzers: Vec::new(),
                        capabilities: Vec::new(),
                        applicability: Applicability::default(),
                        revision: None,
                        tools: Vec::new(),
                    },
                    judge: *judge,
                })
            })
            .collect()
    }
}

fn registry(crash: bool) -> Registry {
    let mut registry = Registry::default();
    let plugin = Plug {
        manifest: PluginManifest {
            id: "toy".to_owned(),
            version: "1".to_owned(),
        },
        crash,
    };
    registry.register(&plugin).unwrap();
    registry
}

/// The pack `toy` with a decision for each rule; `variant` is in the
/// requirement of `toy/pub-then-priv`, so that another variant is a decision
/// that changed.
fn catalog(variant: &str) -> Catalog {
    let mut files: Vec<(String, String)> = vec![(
        "toy/pack.yaml".to_owned(),
        format!(
            "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: toy\nspec:\n  title: Toy\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [{}]\n",
            RULES
                .iter()
                .map(|(id, _)| id.trim_start_matches("toy/"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )];
    for (id, _) in RULES {
        let name = id.trim_start_matches("toy/");
        let requirement = if name == "pub-then-priv" {
            format!("An item MUST come in order {variant}.")
        } else {
            "An item MUST come in order.".to_owned()
        };
        files.push((
            format!("toy/s/{name}.yaml"),
            format!(
                "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: {id}\nspec:\n  title: Order\n  context: Order is read.\n  scope: {{ subject: file }}\n  requirement: {requirement}\n  severity: error\n  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: m\n  examples:\n    - name: ok\n      language: rust\n      kind: valid\n      files:\n        - path: a.rs\n          body: fine\n    - name: bad\n      language: rust\n      kind: invalid\n      files:\n        - path: a.rs\n          body: bad\n      expect:\n        - line: 1\n"
            ),
        ));
    }
    let files: lighthouse_spec::Files = files
        .iter()
        .map(|(path, text)| (path.clone(), text.clone()))
        .collect();
    Catalog::from_files(files, &lighthouse_test_support::cel()).unwrap()
}

fn cell<'a>(catalog: &'a Catalog, language: &'a str, ids: &[&str]) -> Cell<'a> {
    Cell {
        language,
        decisions: ids
            .iter()
            .map(|id| -> (&Decision, Map<String, Value>) {
                (catalog.decision(id).unwrap(), Map::new())
            })
            .collect(),
    }
}

fn graph() -> Graph {
    Graph::in_memory(Options::default()).unwrap()
}

fn search(catalog: &Catalog, graph: &mut Graph, ids: &[&str], bound: usize) -> Searched {
    Oracle::new(|| registry(false), catalog).search(graph, &cell(catalog, "rust", ids), bound)
}

fn bounded(verdict: &Verdict) -> usize {
    match &verdict.basis {
        Basis::Bounded(bounded) => bounded.declarations,
        Basis::Incomplete(why) => panic!("incomplete: {why:?}"),
    }
}

#[test]
fn opposite_orders_have_a_witness_of_two_declarations() {
    let catalog = catalog("a");
    let found = search(
        &catalog,
        &mut graph(),
        &["toy/pub-then-priv", "toy/priv-then-pub"],
        2,
    );

    let [contradiction] = &found.verdict.contradictions[..] else {
        panic!("{:?}", found.verdict);
    };
    assert_eq!(
        contradiction.decisions,
        ["toy/priv-then-pub", "toy/pub-then-priv"]
    );
    let shape = &contradiction.witness.shape;
    assert_eq!(shape.decls.len(), 2);
    assert!(shape.decls.iter().all(|d| d.kind == Kind::Func));
    assert!(shape.edges.is_empty());
    assert!(
        shape
            .decls
            .iter()
            .any(|d| d.visibility == Visibility::Public)
    );
    assert_eq!(contradiction.witness.arrangements.len(), 2);
    assert!(
        contradiction
            .witness
            .arrangements
            .iter()
            .all(|a| a.by.len() == 1)
    );
    assert_eq!(
        contradiction.witness.forced.len(),
        2,
        "each decision forces one order"
    );
    assert_eq!(bounded(&found.verdict), 2);
    assert!(found.verdict.single.is_empty());
}

#[test]
fn a_cycle_of_three_orders_has_a_witness_of_three_declarations() {
    let catalog = catalog("a");
    let ids = [
        "toy/type-then-pub",
        "toy/pub-then-priv",
        "toy/priv-then-type",
    ];
    let found = search(&catalog, &mut graph(), &ids, 3);

    let [contradiction] = &found.verdict.contradictions[..] else {
        panic!("{:?}", found.verdict.contradictions);
    };
    assert_eq!(contradiction.decisions.len(), 3);
    assert_eq!(contradiction.witness.shape.decls.len(), 3);
    assert_eq!(bounded(&found.verdict), 3);
}

#[test]
fn a_nogood_is_minimal_each_decision_alone_flags_an_arrangement_the_others_do_not() {
    let catalog = catalog("a");
    let ids = [
        "toy/type-then-pub",
        "toy/pub-then-priv",
        "toy/priv-then-type",
    ];
    let found = search(&catalog, &mut graph(), &ids, 3);

    let contradiction = &found.verdict.contradictions[0];
    for decision in &contradiction.decisions {
        let alone = contradiction
            .witness
            .arrangements
            .iter()
            .any(|flagged| flagged.by == [decision.clone()]);
        assert!(alone, "{decision} is needed");
    }
}

#[test]
fn compatible_orders_have_no_witness_up_to_the_bound() {
    let catalog = catalog("a");
    let found = search(
        &catalog,
        &mut graph(),
        &["toy/pub-then-priv", "toy/type-then-pub"],
        2,
    );

    assert!(found.verdict.contradictions.is_empty());
    assert_eq!(bounded(&found.verdict), 2);
}

#[test]
fn a_provider_that_crashes_makes_the_search_incomplete_and_keeps_nothing() {
    let catalog = catalog("a");
    let mut graph = graph();
    let ids = ["toy/pub-then-priv", "toy/priv-then-pub"];
    let crashed = Oracle::new(|| registry(true), &catalog).search(
        &mut graph,
        &cell(&catalog, "rust", &ids),
        2,
    );

    assert!(
        matches!(crashed.verdict.basis, Basis::Incomplete(_)),
        "{:?}",
        crashed.verdict
    );
    assert!(crashed.verdict.contradictions.is_empty());
    let again = search(&catalog, &mut graph, &ids, 2);
    assert_eq!(again.ran.len(), 2, "what a crash gave was not kept");
    assert_eq!(again.verdict.contradictions.len(), 1);
}

#[test]
fn a_language_without_a_renderer_is_incomplete() {
    let catalog = catalog("a");
    let found = Oracle::new(|| registry(false), &catalog).search(
        &mut graph(),
        &cell(&catalog, "python", &["toy/pub-then-priv"]),
        2,
    );

    let Basis::Incomplete(why) = &found.verdict.basis else {
        panic!("{:?}", found.verdict);
    };
    assert!(why[0].contains("no renderer for `python`"), "{why:?}");
    assert!(found.verdict.contradictions.is_empty());
}

#[test]
fn unchanged_decisions_are_not_run_again() {
    let catalog = catalog("a");
    let mut graph = graph();
    let ids = ["toy/pub-then-priv", "toy/priv-then-pub"];
    let first = search(&catalog, &mut graph, &ids, 2);
    let second = search(&catalog, &mut graph, &ids, 2);

    assert_eq!(first.ran.len(), 2);
    assert!(second.ran.is_empty());
    assert_eq!(first.verdict, second.verdict);
}

#[test]
fn flags_are_kept_in_the_store_between_processes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("graph.db");
    let catalog = catalog("a");
    let ids = ["toy/pub-then-priv", "toy/priv-then-pub"];
    let first = {
        let mut graph = Graph::open(&db, Options::default()).unwrap();
        search(&catalog, &mut graph, &ids, 2)
    };
    let mut graph = Graph::open(&db, Options::default()).unwrap();
    let second = search(&catalog, &mut graph, &ids, 2);

    assert_eq!(first.ran.len(), 2);
    assert!(second.ran.is_empty(), "{:?}", second.ran);
    assert_eq!(first.verdict, second.verdict);
}

#[test]
fn a_changed_decision_is_run_again_and_the_others_are_not_and_the_verdict_is_that_of_a_fresh_graph()
{
    let before = catalog("a");
    let after = catalog("b");
    let ids = ["toy/pub-then-priv", "toy/priv-then-pub"];
    let mut kept = graph();
    search(&before, &mut kept, &ids, 2);

    let incremental = search(&after, &mut kept, &ids, 2);
    let scratch = search(&after, &mut graph(), &ids, 2);

    assert_eq!(incremental.ran, ["toy/pub-then-priv"]);
    assert_eq!(
        serde_json::to_string(&incremental.verdict).unwrap(),
        serde_json::to_string(&scratch.verdict).unwrap()
    );
}

#[test]
fn two_runs_give_byte_identical_verdicts() {
    let catalog = catalog("a");
    let ids = [
        "toy/type-then-pub",
        "toy/pub-then-priv",
        "toy/priv-then-type",
    ];
    let one = search(&catalog, &mut graph(), &ids, 3);
    let two = search(&catalog, &mut graph(), &ids, 3);

    assert_eq!(
        serde_json::to_string(&one.verdict).unwrap(),
        serde_json::to_string(&two.verdict).unwrap()
    );
}

#[test]
fn a_decision_the_alphabet_cannot_tell_arrangements_by_makes_the_search_incomplete() {
    let catalog = catalog("a");
    let found = search(
        &catalog,
        &mut graph(),
        &["toy/never", "toy/pub-then-priv"],
        2,
    );

    let Basis::Incomplete(why) = &found.verdict.basis else {
        panic!("{:?}", found.verdict);
    };
    assert!(
        why.iter().any(|reason| reason.contains("`toy/never`")),
        "{why:?}"
    );
    assert!(
        !why.iter()
            .any(|reason| reason.contains("`toy/pub-then-priv`")),
        "{why:?}"
    );
}

#[test]
fn a_decision_that_flags_a_shape_alone_is_never_a_contradiction() {
    let catalog = catalog("a");
    let found = search(
        &catalog,
        &mut graph(),
        &["toy/always", "toy/pub-then-priv"],
        2,
    );

    assert!(
        found.verdict.contradictions.is_empty(),
        "{:?}",
        found.verdict.contradictions
    );
    assert!(
        found
            .verdict
            .single
            .iter()
            .any(|single| single.decision == "toy/always")
    );
    assert!(matches!(found.verdict.basis, Basis::Incomplete(_)));
}

#[test]
fn a_finding_outside_the_shapes_makes_the_search_incomplete() {
    let catalog = catalog("a");
    let found = search(
        &catalog,
        &mut graph(),
        &["toy/stray", "toy/pub-then-priv"],
        2,
    );

    let Basis::Incomplete(why) = &found.verdict.basis else {
        panic!("{:?}", found.verdict);
    };
    assert!(
        why[0].contains("finding outside the shapes: toy/stray"),
        "{why:?}"
    );
    assert!(found.verdict.contradictions.is_empty());
    assert!(found.ran.is_empty(), "nothing was kept");
}

#[test]
fn a_decision_that_is_unsatisfiable_alone_keeps_its_pairs_out() {
    let catalog = catalog("a");
    let ids = ["toy/always", "toy/pub-then-priv", "toy/priv-then-pub"];
    let found = search(&catalog, &mut graph(), &ids, 2);

    let singles: Vec<&str> = found
        .verdict
        .single
        .iter()
        .map(|s| s.decision.as_str())
        .collect();
    assert_eq!(singles, ["toy/always"]);
    let pairs: Vec<&Vec<String>> = found
        .verdict
        .contradictions
        .iter()
        .map(|c| &c.decisions)
        .collect();
    assert_eq!(
        pairs,
        [&vec![
            "toy/priv-then-pub".to_owned(),
            "toy/pub-then-priv".to_owned()
        ]]
    );
}

#[test]
fn oracle_sharing_searches_over_the_registry_it_is_given() {
    let catalog = catalog("a");
    let ids = ["toy/pub-then-priv", "toy/priv-then-pub"];
    let sharing = Oracle::sharing(std::sync::Arc::new(registry(false)), &catalog);

    let found = sharing.search(&mut graph(), &cell(&catalog, "rust", &ids), 2);

    let own = search(&catalog, &mut graph(), &ids, 2);
    assert_eq!(found.verdict, own.verdict);
    assert_eq!(found.verdict.contradictions.len(), 1);
}
