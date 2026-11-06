//! What the facts of rules are made of: the fields the rules read, merged by
//! the sort of subject, and the plugin that supplies the queries.

use std::{collections::BTreeSet, sync::Arc};

use lighthouse_graph::Graph;
use lighthouse_model::{Diagnostic, Options, RunScope, Severity, ucm::Applicability};
use lighthouse_plugin::{
    Context, Error, NoQueries, Plugin, PluginManifest, Pulling, Queries, Reads, Registry, Rule,
    RuleManifest, Workspace,
};

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

struct Reading {
    manifest: RuleManifest,
    reads: Option<(&'static str, Reads)>,
}

impl Reading {
    fn of(subject: Option<&'static str>, fields: &[&str]) -> Self {
        Self {
            manifest: RuleManifest {
                id: "t/r".to_owned(),
                uid: None,
                severity: Severity::Warn,
                scope: RunScope::File,
                description: String::new(),
                docs: String::new(),
                analyzers: Vec::new(),
                capabilities: Vec::new(),
                applicability: Applicability::default(),
                revision: None,
                tools: Vec::new(),
            },
            reads: subject.map(|subject| {
                (
                    subject,
                    Reads {
                        fields: set(fields),
                        ..Reads::default()
                    },
                )
            }),
        }
    }
}

impl Rule for Reading {
    fn manifest(&self) -> &RuleManifest {
        &self.manifest
    }
    fn validate(&self, _: &Options) -> Result<(), Error> {
        Ok(())
    }
    fn check(&self, _: &Context, _: &Options) -> Result<Vec<Diagnostic>, Error> {
        Ok(Vec::new())
    }
    fn reads(&self) -> Option<(&'static str, Reads)> {
        self.reads.clone()
    }
}

struct Pulled(PluginManifest);

impl Plugin for Pulled {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn pulling(&self) -> Option<Arc<dyn Pulling>> {
        Some(Arc::new(Nothing))
    }
}

struct Nothing;

impl Pulling for Nothing {
    fn inputs(
        &self,
        _: &mut Graph,
        _: &[&dyn Rule],
        _: &Workspace,
    ) -> Result<(), lighthouse_graph::Error> {
        Ok(())
    }
    fn queries<'g>(&self, _: &'g Graph) -> Box<dyn Queries + 'g> {
        Box::new(NoQueries)
    }
}

#[test]
fn reads_merge() {
    let mut reads = Reads {
        fields: set(&["name"]),
        ..Reads::default()
    };

    reads.merge(&Reads {
        fields: set(&["doc"]),
        related: set(&["callers"]),
        dynamic: true,
        related_dynamic: false,
    });
    reads.merge(&Reads::default());

    assert_eq!(reads.fields, set(&["doc", "name"]));
    assert_eq!(reads.related, set(&["callers"]));
    assert!(reads.dynamic);
    assert!(!reads.related_dynamic);
}

#[test]
fn reads_of() {
    let rules = [
        Reading::of(Some("symbol"), &["name"]),
        Reading::of(Some("symbol"), &["doc"]),
        Reading::of(Some("test"), &["name"]),
        Reading::of(None, &[]),
    ];

    let found = lighthouse_plugin::reads_of(rules.iter().map(|rule| rule as &dyn Rule));

    assert_eq!(found.len(), 2);
    assert_eq!(found["symbol"].fields, set(&["doc", "name"]));
    assert_eq!(found["test"].fields, set(&["name"]));
}

#[test]
fn registry_pulling() {
    let mut registry = Registry::default();
    assert!(registry.pulling().is_none());

    registry
        .register(&Pulled(PluginManifest {
            id: "t".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();

    assert!(registry.pulling().is_some());
}

#[test]
fn analyzer_context_fact() {
    let mut facts = lighthouse_plugin::Facts::new();
    facts.insert("a/count", String::new(), serde_json::json!(3));
    let model = lighthouse_model::ucm::Project::default();
    let ctx = lighthouse_plugin::AnalyzerContext {
        model: &model,
        file: None,
        facts: &facts,
    };

    assert_eq!(ctx.fact::<u32>("a/count").unwrap(), 3);
    assert!(matches!(
        ctx.fact::<u32>("a/missing"),
        Err(Error::MissingFact(_))
    ));
}
