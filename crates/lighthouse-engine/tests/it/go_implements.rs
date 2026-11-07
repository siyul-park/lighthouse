//! A Go type satisfies an interface of a package it does not import: the
//! provider cannot tell, so the host settles it from the method sets of every
//! package of the language.

use std::{fs, path::Path};

use lighthouse_engine::Engine;
use lighthouse_model::ucm::{EdgeKind, Model, Node, Project, SymbolId, Target};
use lighthouse_spec::Config;

const CONFORMANCE: &str = "../lighthouse-rpc/tests/conformance/go";

fn copy(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The model the engine makes of the conformance case `case`; `None` without
/// a Go toolchain.
fn modelled(case: &str) -> Option<Project> {
    let plugin = lighthouse_test_support::lang_go()?;
    let project = tempfile::tempdir().unwrap();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(CONFORMANCE)
            .join(case)
            .join("project"),
        project.path(),
    );
    let config = Config::parse_inline(&format!(
        "plugins = [{{ id = \"lang-go\", path = {:?} }}]\n",
        plugin.display().to_string()
    ))
    .unwrap();
    let registry = lighthouse_test_support::pooled(lighthouse_test_support::registry, &config)();
    let config = config.enabling(registry.manifests().iter().map(|m| m.id.as_str()));
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        project.path(),
    )
    .unwrap();
    Some(engine.check_project(&[], &[]).unwrap().project)
}

fn id(text: &str) -> SymbolId {
    SymbolId::parse(text).unwrap()
}

/// Whether a type has an `implements` edge to an interface.
fn implements(project: &Project, from: &str, to: &str) -> bool {
    let (from, to) = (id(from), id(to));
    project.edges_from(&from).any(|edge| {
        edge.kind == EdgeKind::Implements && edge.to == Target::Resolved(Node::Symbol(to.clone()))
    })
}

fn settled(case: &str, interface: &str) {
    let Some(project) = modelled(case) else {
        return;
    };
    for method in ["Close", "Flush"] {
        let method = id(&format!("impl::File::{method}#method"));
        assert!(
            project.symbol(&method).unwrap().implementation,
            "{method:?}"
        );
    }
    assert!(implements(&project, "impl::File#type", interface));
    assert_eq!(
        project.implementers(&id(interface)),
        [id("impl::File#type")]
    );
}

#[test]
fn a_type_satisfies_the_interface_of_a_package_it_does_not_import() {
    settled("unimported", "api::Sink#interface");
}

#[test]
fn an_interface_only_a_test_file_of_another_package_declares_counts() {
    settled("tested", "api::sink#interface");
}
