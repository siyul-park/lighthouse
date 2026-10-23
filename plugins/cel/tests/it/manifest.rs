//! The manifests against what the libraries register.

use cel::{Context, ExecutionError, Value};
use lighthouse_cel::Cel;
use lighthouse_plugin::Registry;

use crate::ext::environment;

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry.register(&Cel::new()).unwrap();
    registry
}

#[test]
fn the_plugin_registers_without_a_duplicate_function() {
    let registry = registry();

    assert!(!registry.cel_functions().is_empty());
    assert!(
        registry
            .cel_libraries()
            .all(|l| l.manifest().id.starts_with("cel/"))
    );
}

#[test]
fn every_function_of_the_catalog_is_registered_with_every_overload_it_lists() {
    let env = environment();
    let context = Context::with_env(env.clone());
    let registry = registry();
    for entry in registry.cel_functions() {
        let function = entry.function;
        if function.name == "cel.bind" {
            // A macro: it is expanded when the source is compiled.
            continue;
        }
        for overload in &function.overloads {
            let args = vec!["null"; overload.params.len()].join(", ");
            let source = match &function.receiver {
                Some(_) => format!("null.{}({args})", function.name),
                None => format!("{}({args})", function.name),
            };
            let probe = env.compile(&source).unwrap();
            let outcome = probe.execute(&context);
            assert!(
                !matches!(outcome, Err(ExecutionError::UndeclaredReference(_))),
                "{source}: {outcome:?}"
            );
        }
    }
}

#[test]
fn every_example_of_the_catalog_holds() {
    let env = environment();
    let context = Context::with_env(env.clone());
    let registry = registry();
    for entry in registry.cel_functions() {
        let function = entry.function;
        let outcome = env
            .compile(&function.example)
            .unwrap()
            .execute(&context)
            .unwrap();
        assert_eq!(outcome, Value::Bool(true), "{}", function.example);
    }
}

#[test]
fn every_library_is_named_after_the_plugin_and_has_a_version() {
    let registry = registry();
    for library in registry.cel_libraries() {
        let manifest = library.manifest();
        assert!(manifest.id.starts_with("cel/"), "{}", manifest.id);
        assert!(!manifest.version.is_empty());
    }
}
