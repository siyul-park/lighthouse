use cel::Env;
use lighthouse_plugin::{
    CelLibrary, CelLibraryManifest, Error, Function, Overload, Plugin, PluginManifest, Precedence,
    Registry,
};

struct Library(CelLibraryManifest);

impl CelLibrary for Library {
    fn manifest(&self) -> &CelLibraryManifest {
        &self.0
    }

    fn register(&self, _: &mut Env) -> Result<(), Error> {
        Ok(())
    }
}

fn function(name: &str, receiver: Option<&str>, params: &[&[&str]]) -> Function {
    Function {
        name: name.to_owned(),
        receiver: receiver.map(str::to_owned),
        overloads: params.iter().map(|p| Overload::new(p, "int")).collect(),
        doc: String::new(),
        example: String::new(),
        precedence: Precedence::Designed,
    }
}

struct Contributor {
    manifest: PluginManifest,
    libraries: Vec<(String, Vec<Function>)>,
}

impl Contributor {
    fn new(plugin: &str, libraries: Vec<(&str, Vec<Function>)>) -> Self {
        Self {
            manifest: PluginManifest {
                id: plugin.to_owned(),
                version: "1".to_owned(),
            },
            libraries: libraries
                .into_iter()
                .map(|(id, functions)| (id.to_owned(), functions))
                .collect(),
        }
    }
}

impl Plugin for Contributor {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn cel_libraries(&self) -> Vec<Box<dyn CelLibrary>> {
        self.libraries
            .iter()
            .map(|(id, functions)| {
                Box::new(Library(CelLibraryManifest {
                    id: id.clone(),
                    version: "1.0.0".to_owned(),
                    functions: functions.clone(),
                })) as Box<dyn CelLibrary>
            })
            .collect()
    }
}

#[test]
fn the_catalog_is_the_union_of_the_libraries_sorted_by_name() {
    let mut registry = Registry::default();
    registry
        .register(&Contributor::new(
            "a",
            vec![
                ("a/one", vec![function("zeta", None, &[&["int"]])]),
                ("a/two", vec![function("alpha", Some("list"), &[&[]])]),
            ],
        ))
        .unwrap();
    registry
        .register(&Contributor::new(
            "b",
            vec![("b/one", vec![function("mid", None, &[&[]])])],
        ))
        .unwrap();

    let names: Vec<(&str, &str)> = registry
        .cel()
        .functions()
        .iter()
        .map(|c| (c.function.name.as_str(), c.library))
        .collect();

    assert_eq!(
        names,
        [("alpha", "a/two"), ("mid", "b/one"), ("zeta", "a/one")]
    );
}

#[test]
fn a_function_two_libraries_provide_is_a_load_error_and_nothing_of_the_plugin_is_kept() {
    let mut registry = Registry::default();
    registry
        .register(&Contributor::new(
            "a",
            vec![("a/one", vec![function("same", None, &[&["int"]])])],
        ))
        .unwrap();

    let error = registry
        .register(&Contributor::new(
            "b",
            vec![("b/one", vec![function("same", None, &[&["int"]])])],
        ))
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("`same` is provided by both `a/one` and `b/one`"),
        "{error}"
    );
    assert!(!registry.has_plugin("b"));
    assert_eq!(registry.cel().functions().len(), 1);
}

#[test]
fn the_same_name_on_another_kind_of_value_is_another_function() {
    let mut registry = Registry::default();
    registry
        .register(&Contributor::new(
            "a",
            vec![
                ("a/one", vec![function("size", None, &[&["int"]])]),
                ("a/two", vec![function("size", Some("list"), &[&[]])]),
            ],
        ))
        .unwrap();

    assert_eq!(registry.cel().functions().len(), 2);
}

#[test]
fn a_function_with_an_overload_twice_is_a_load_error() {
    let mut registry = Registry::default();

    let error = registry
        .register(&Contributor::new(
            "a",
            vec![(
                "a/one",
                vec![function("twice", None, &[&["int"], &["int"]])],
            )],
        ))
        .unwrap_err();

    assert!(
        error.to_string().contains("overload (int) twice"),
        "{error}"
    );
}

#[test]
fn a_library_is_named_after_its_plugin_and_once() {
    let mut registry = Registry::default();
    let error = registry
        .register(&Contributor::new("a", vec![("b/one", vec![])]))
        .unwrap_err();
    assert!(matches!(error, Error::Prefix { .. }), "{error}");

    registry
        .register(&Contributor::new("a", vec![("a/one", vec![])]))
        .unwrap();
    let error = registry
        .register(&Contributor::new("c", vec![("a/one", vec![])]))
        .unwrap_err();
    assert!(matches!(error, Error::Prefix { .. }), "{error}");
    let both = Contributor::new("d", vec![("d/x", vec![]), ("d/x", vec![])]);
    assert!(matches!(
        registry.register(&both).unwrap_err(),
        Error::Duplicate(_)
    ));
}

#[test]
fn precedence_step() {
    assert_eq!(Precedence::Official.step(), 2);
    assert_eq!(Precedence::Designed.step(), 3);
}

#[test]
fn function_namespace() {
    assert_eq!(
        function("regex.replace", None, &[]).namespace(),
        Some("regex")
    );
    assert_eq!(function("indexOf", Some("list"), &[]).namespace(), None);
}

#[test]
fn function_accepts() {
    let f = function("f", None, &[&["int"], &["int", "int"]]);
    assert!(f.accepts(1) && f.accepts(2));
    assert!(!f.accepts(0) && !f.accepts(3));
}
