use super::*;
use lighthouse_protocol::TextMapSegment;

fn entry(range: [u32; 2]) -> lighthouse_plugin::Mapping {
    lighthouse_plugin::Mapping {
        range: Some(range),
        text_pointer: None,
        text_map: None,
    }
}

#[test]
fn selectors_preserve_json_pointers_when_values_repeat() {
    let ast = Source {
        path: "doc.yaml".to_owned(),
        ast: serde_json::json!({"items":["same", "same"]}),
        mapping: BTreeMap::from([
            ("".to_owned(), entry([0, 9])),
            ("/items".to_owned(), entry([0, 9])),
            ("/items/0".to_owned(), entry([0, 4])),
            ("/items/1".to_owned(), entry([5, 9])),
        ]),
    };
    let selected = evaluate(&ast, "", "$.items[*]").unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|item| item.pointer.as_str())
            .collect::<Vec<_>>(),
        ["/items/0", "/items/1"]
    );
    assert_eq!(selected[0].value, selected[1].value);
}

#[test]
fn descendants_come_parents_first_with_their_pointers() {
    let ast = Source {
        path: "doc.md".to_owned(),
        ast: serde_json::json!({
            "kind": "document",
            "children": [
                {"kind": "code_block", "attributes": []},
                {"kind": "list", "children": [
                    {"kind": "item", "children": [
                        {"kind": "code_block", "attributes": ["ignore"]}
                    ]}
                ]}
            ]
        }),
        mapping: BTreeMap::new(),
    };
    let blocks = evaluate(&ast, "", "$..?(@.kind == 'code_block')").unwrap();
    let pointers: Vec<_> = blocks.iter().map(|item| item.pointer.as_str()).collect();
    assert_eq!(
        pointers,
        ["/children/0", "/children/1/children/0/children/0"]
    );
    let kept = evaluate(
        &ast,
        "",
        "$..?(@.kind == 'code_block' && @.attributes[*] != 'ignore')",
    )
    .unwrap();
    let pointers: Vec<_> = kept.iter().map(|item| item.pointer.as_str()).collect();
    assert_eq!(pointers, ["/children/0"]);
}

#[test]
fn source_projection_uses_text_pointer_without_searching_values() {
    let host = "expr: x > 1";
    let text_map = vec![TextMapSegment {
        text_range: [0, 5],
        source_range: [6, 11],
    }];
    let ast = Source {
        path: "doc.yaml".to_owned(),
        ast: serde_json::json!({"expr-node":{"kind":"Expression","source":"x > 1"}}),
        mapping: BTreeMap::from([
            ("".to_owned(), entry([0, 11])),
            (
                "/expr-node".to_owned(),
                lighthouse_plugin::Mapping {
                    range: Some([0, 11]),
                    text_pointer: Some("/expr-node/source".to_owned()),
                    text_map: None,
                },
            ),
            ("/expr-node/kind".to_owned(), entry([0, 0])),
            (
                "/expr-node/source".to_owned(),
                lighthouse_plugin::Mapping {
                    range: Some([6, 11]),
                    text_pointer: None,
                    text_map: Some(text_map),
                },
            ),
        ]),
    };
    assert!(ast.validate(host).is_ok());
    let rule = InjectionSpec {
        id: "embedded".to_owned(),
        files: vec!["**/*.yaml".to_owned()],
        select: "$.expr-node".to_owned(),
        language: Some("cel".to_owned()),
        language_from: None,
        syntax: None,
        context: None,
    };
    let result = select_rule(&Injection::new(&rule), &ast, &|_| None).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].node_pointer, "/expr-node");
    assert_eq!(result[0].source_pointer, "/expr-node/source");
    assert_eq!(result[0].text, "x > 1");
    assert_eq!(result[0].source_range, [6, 11]);
}

#[test]
fn context_selectors_use_the_documented_scopes() {
    let text = "Decision cel x > 1";
    let ast = Source {
        path: "decisions/example.yaml".to_owned(),
        ast: serde_json::json!({
            "kind":"Decision",
            "language":"cel",
            "where":"x > 1",
        }),
        mapping: BTreeMap::from([
            ("".to_owned(), entry([0, text.len() as u32])),
            ("/kind".to_owned(), entry([0, 8])),
            ("/language".to_owned(), entry([9, 12])),
            ("/where".to_owned(), entry([13, 18])),
        ]),
    };
    let rule = InjectionSpec {
        id: "where".to_owned(),
        files: vec!["decisions/**/*.yaml".to_owned()],
        select: "$?(@.kind == 'Decision' && @.language == 'cel').where".to_owned(),
        language: Some("cel".to_owned()),
        language_from: None,
        syntax: None,
        context: Some(serde_json::json!({
            "identity": {"kind":"$.kind"},
            "declared": true
        })),
    };
    let result = select_rule(&Injection::new(&rule), &ast, &|_| None).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].provider, "cel");
    assert_eq!(result[0].text, "x > 1");
    assert_eq!(result[0].context["identity"]["kind"], "Decision");
}

#[test]
fn root_identity_selectors_are_scoped_to_the_candidate_document() {
    let ast = Source {
        path: "multi.yaml".to_owned(),
        ast: serde_json::json!({
            "kind": "Stream",
            "documents": [
                {"kind":"Decision","metadata":{"name":"first"},"spec":{"where":"one"}},
                {"kind":"Decision","metadata":{"name":"second"},"spec":{"where":"two"}}
            ]
        }),
        mapping: BTreeMap::new(),
    };
    let candidate = Selected {
        value: pointer_value(&ast.ast, "/documents/1/spec/where").unwrap(),
        pointer: "/documents/1/spec/where".to_owned(),
    };

    let kinds = identity_values(&ast, &candidate, "$.documents[*].kind").unwrap();
    let names = identity_values(&ast, &candidate, "$.documents[*].metadata.name").unwrap();

    assert_eq!(kinds.len(), 1);
    assert_eq!(kinds[0].pointer, "/documents/1/kind");
    assert_eq!(kinds[0].value, "Decision");
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].pointer, "/documents/1/metadata/name");
    assert_eq!(names[0].value, "second");
}

#[test]
fn unsupported_composite_candidates_are_errors_not_empty_success() {
    let ast = Source {
        path: "doc.yaml".to_owned(),
        ast: serde_json::json!({"expr":{"nested":"not an explicit source pointer"}}),
        mapping: BTreeMap::from([
            ("".to_owned(), entry([0, 10])),
            ("/expr".to_owned(), entry([0, 10])),
            ("/expr/nested".to_owned(), entry([2, 9])),
        ]),
    };
    let rule = InjectionSpec {
        id: "embedded".to_owned(),
        files: vec!["**/*.yaml".to_owned()],
        select: "$.expr".to_owned(),
        language: Some("cel".to_owned()),
        language_from: None,
        syntax: None,
        context: None,
    };
    assert!(
        select_rule(&Injection::new(&rule), &ast, &|_| None)
            .unwrap_err()
            .contains("no explicit text_pointer")
    );
}

/// A markdown-shaped AST: the code blocks of a document, each with the
/// language and info string of its fence and the text of its body.
fn document(blocks: &[(&str, &str, &[&str])]) -> Source {
    let mut mapping = BTreeMap::from([("".to_owned(), entry([0, 1000]))]);
    let mut children = Vec::new();
    for (index, (lang, info, attributes)) in blocks.iter().enumerate() {
        let at = u32::try_from(index).unwrap() * 100;
        let node = format!("/children/{index}");
        mapping.insert(
            node.clone(),
            lighthouse_plugin::Mapping {
                range: Some([at, at + 90]),
                text_pointer: Some(format!("{node}/text")),
                text_map: None,
            },
        );
        mapping.insert(format!("{node}/text"), entry([at + 10, at + 20]));
        mapping.insert(format!("{node}/lang"), entry([at + 3, at + 6]));
        mapping.insert(format!("{node}/info"), entry([at + 3, at + 9]));
        let mut block = serde_json::json!({
            "kind": "code_block",
            "info": info,
            "attributes": attributes,
            "text": "fn a() {}\n",
        });
        if !lang.is_empty() {
            block["lang"] = serde_json::json!(lang);
        }
        children.push(block);
    }
    Source {
        path: "README.md".to_owned(),
        ast: serde_json::json!({"kind": "document", "children": children}),
        mapping,
    }
}

fn markdown_rule() -> InjectionSpec {
    let config = lighthouse_spec::Config::parse(include_str!("../../../../../lighthouse.toml"))
        .expect("the project's own configuration parses");
    config
        .injections()
        .iter()
        .find(|rule| rule.id == "markdown-code")
        .expect("the project reads the fences of its Markdown")
        .clone()
}

fn named(name: &str) -> Option<String> {
    match name.to_lowercase().as_str() {
        "rust" | "rs" => Some("rust".to_owned()),
        "go" | "golang" => Some("go".to_owned()),
        _ => None,
    }
}

fn analysed(blocks: &[(&str, &str, &[&str])]) -> Vec<(String, Value)> {
    let rule = markdown_rule();
    select_rule(&Injection::new(&rule), &document(blocks), &named)
        .unwrap()
        .into_iter()
        .map(|source| (source.provider, source.context))
        .collect()
}

#[test]
fn the_fences_of_a_language_with_a_name_are_analysed_by_its_provider_as_examples() {
    let found = analysed(&[
        ("rust", "rust", &[]),
        ("GoLang", "GoLang", &[]),
        ("", "", &[]),
        ("cobol", "cobol", &[]),
    ]);

    let providers: Vec<_> = found
        .iter()
        .map(|(provider, _)| provider.as_str())
        .collect();
    assert_eq!(
        providers,
        ["rust", "go"],
        "no name and an unknown name are text"
    );
    assert_eq!(found[0].1["example"], true);
    assert_eq!(found[0].1["info"], "rust");
}

#[test]
fn the_attributes_that_say_a_fence_is_not_code_to_check_leave_it_out() {
    let found = analysed(&[
        ("rust", "rust,ignore", &["ignore"]),
        ("rust", "rust,compile_fail", &["compile_fail"]),
        ("rust", "rust,invalid", &["invalid"]),
        ("rust", "rust,no_run", &["no_run"]),
        ("rust", "rust,edition2021", &["edition2021"]),
    ]);

    let infos: Vec<_> = found
        .iter()
        .map(|(_, context)| context["info"].clone())
        .collect();
    assert_eq!(infos, ["rust,no_run", "rust,edition2021"]);
}

#[test]
fn a_fence_in_a_list_is_found_below_the_top_of_the_document() {
    let flat = document(&[("rust", "rust", &[])]);
    let nest = |pointer: &str| pointer.replacen("/children/0", "/children/0/children/0", 1);
    let ast = Source {
        path: flat.path.clone(),
        ast: serde_json::json!({"kind": "document", "children": [
            {"kind": "list", "children": [flat.ast["children"][0].clone()]}
        ]}),
        mapping: flat
            .mapping
            .iter()
            .map(|(pointer, mapping)| {
                let mut mapping = mapping.clone();
                mapping.text_pointer = mapping.text_pointer.as_deref().map(nest);
                (nest(pointer), mapping)
            })
            .collect(),
    };
    let rule = markdown_rule();

    let found = select_rule(&Injection::new(&rule), &ast, &named).unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].node_pointer, "/children/0/children/0");
}

/// The document of RFC 9535 section 2.5.2 (descendant segment).
fn rfc_document() -> Source {
    Source {
        path: "rfc.json".to_owned(),
        ast: serde_json::json!({"o": {"j": 1, "k": 2}, "a": [5, 3, [{"j": 4}, {"k": 6}]]}),
        mapping: BTreeMap::new(),
    }
}

fn found(query: &str) -> Vec<Value> {
    evaluate(&rfc_document(), "", query)
        .unwrap()
        .into_iter()
        .map(|item| item.value.clone())
        .collect()
}

#[test]
fn the_descendant_segment_applies_the_next_selector_to_every_node_below() {
    // The members of an object come in the order of their source; this
    // document has no mapping, so any order is right.
    let mut js = found("$..j");
    js.sort_by_key(|value| value.as_i64());
    assert_eq!(js, [serde_json::json!(1), serde_json::json!(4)]);
    assert_eq!(
        found("$..[0]"),
        [serde_json::json!(5), serde_json::json!({"j": 4})]
    );
    // Unlike RFC 9535's, `[*]` takes the elements of arrays and `.*` the
    // members of objects.
    assert_eq!(found("$..[*]").len(), 5, "the elements of the three arrays");
    assert_eq!(found("$..*").len(), 6, "the members of the four objects");
}

#[test]
fn a_filter_on_a_path_that_selects_nothing_holds_for_inequality_only() {
    let hits = |query: &str| found(query).len();

    assert_eq!(hits("$..?(@.j == 4)"), 1);
    assert_eq!(hits("$..?(@.zz == 4)"), 0);
    assert_eq!(hits("$..?(@.zz != 4)"), hits("$..?(@.k != 99)"));
}

#[test]
fn a_union_selects_the_members_it_names_in_its_order_below_each_match() {
    let ast = Source {
        path: "doc.toml".to_owned(),
        ast: serde_json::json!({"events": {"each": "a", "select": "b", "other": "c"}, "list": ["x", "y"]}),
        mapping: BTreeMap::new(),
    };
    let named = |query: &str| -> Vec<String> {
        evaluate(&ast, "", query)
            .unwrap()
            .into_iter()
            .map(|item| item.pointer)
            .collect()
    };

    assert_eq!(
        named("$.events['select','absent','each']"),
        ["/events/select", "/events/each"]
    );
    assert_eq!(named("$.list[1,0]"), ["/list/1", "/list/0"]);
    assert_eq!(
        named("$..['select','each']"),
        ["/events/select", "/events/each"],
        "after a descent the union names members at any depth"
    );
}

/// The `sources` this repository declared before `core/authoring` shared
/// them, as injections.
const BEFORE: &str = include_str!("before.toml");

/// What an injection selected, without the id of the injection that did.
type Region = (PathBuf, [u32; 2], String, Option<String>, String, String);

/// The regions `injections` select from the files of the repository, from the
/// ASTs the language plugins build of them, sorted.
fn repository_regions(injections: &[InjectionSpec], asts: &[Source], root: &Path) -> Vec<Region> {
    let inputs: Vec<Input> = asts
        .iter()
        .map(|ast| Input {
            file: lighthouse_model::ucm::File {
                path: PathBuf::from(&ast.path),
                lang: String::new(),
                hash: String::new(),
                generated: false,
                test: false,
                facts: BTreeMap::new(),
            },
            text: std::fs::read_to_string(root.join(&ast.path)).unwrap(),
            language: 0,
        })
        .collect();
    let named = |name: &str| Some(name.to_lowercase());
    let mut run = Run::default();
    let selected = select(injections, asts.to_vec(), &inputs, &named, &mut run);
    assert!(run.incomplete.is_empty(), "{:?}", run.incomplete);
    let mut regions: Vec<Region> = selected
        .into_iter()
        .flat_map(|file| {
            let path = file.file.path.clone();
            file.regions.into_iter().map(move |source| {
                (
                    path.clone(),
                    source.source_range,
                    source.provider,
                    source.syntax,
                    source.context.to_string(),
                    source.text,
                )
            })
        })
        .collect();
    regions.sort();
    regions
}

/// The files of the repository an injection of either set reads, by path.
fn host_files(globs: &[String], root: &Path) -> Vec<PathBuf> {
    let matcher = lighthouse_spec::glob_set(globs).unwrap();
    let skipped = ["target", "node_modules", ".git", "dist"];
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .filter_entry(move |entry| !skipped.iter().any(|name| entry.file_name() == *name))
        .build();
    let mut found: Vec<PathBuf> = walk
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.path().strip_prefix(root).ok().map(Path::to_owned))
        .filter(|path| matcher.is_match(path))
        .collect();
    found.sort();
    found
}

/// The ASTs the language plugins give the files, which they are asked for by
/// the globs of the injections.
fn repository_asts(files: &[PathBuf], globs: &[String], root: &Path) -> Vec<Source> {
    let config = lighthouse_spec::Config::parse_inline(&format!(
        "plugins = [{}]",
        [
            ("lang-yaml", lighthouse_test_support::lang_yaml()),
            ("lang-toml", lighthouse_test_support::lang_toml()),
            ("lang-markdown", lighthouse_test_support::lang_markdown()),
        ]
        .map(|(id, dir)| format!(
            "{{ id = \"{id}\", path = {:?} }}",
            dir.display().to_string()
        ))
        .join(", ")
    ))
    .unwrap();
    let registry = lighthouse_test_support::pooled(lighthouse_plugin::Registry::default, &config)();
    let workspace = lighthouse_plugin::Workspace {
        root: root.to_owned(),
        languages: BTreeMap::new(),
        source_globs: globs.to_vec(),
        constructors: BTreeMap::new(),
        test_naming: BTreeMap::new(),
        overlays: BTreeMap::new(),
        cel_libraries: Vec::new(),
        cache_dir: None,
        sandbox: lighthouse_process::Policy::default(),
    };
    let mut asts = Vec::new();
    for language in registry.languages() {
        let provider = &language.provider;
        let manifest = provider.manifest();
        let own = lighthouse_spec::glob_set(&manifest.globs).unwrap();
        let texts: Vec<(lighthouse_model::ucm::File, String)> = files
            .iter()
            .filter(|path| own.is_match(path))
            .map(|path| {
                let file = lighthouse_model::ucm::File {
                    path: path.clone(),
                    lang: manifest.id.clone(),
                    hash: String::new(),
                    generated: false,
                    test: false,
                    facts: BTreeMap::new(),
                };
                (file, std::fs::read_to_string(root.join(path)).unwrap())
            })
            .collect();
        let sources: Vec<lighthouse_plugin::Source<'_>> = texts
            .iter()
            .map(|(file, text)| lighthouse_plugin::Source {
                file,
                text,
                regions: &[],
            })
            .collect();
        asts.extend(provider.index(&workspace, &sources).unwrap().sources);
    }
    asts
}

#[test]
fn core_authoring_selects_the_regions_the_repository_had_before_it_shared_them() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let before = lighthouse_spec::Config::parse_inline(BEFORE).unwrap();
    let now = lighthouse_spec::Config::parse(include_str!("../../../../../lighthouse.toml"))
        .unwrap()
        .inheriting(lighthouse_test_support::full().projects())
        .unwrap();
    let globs: Vec<String> = before
        .injections()
        .iter()
        .chain(now.injections())
        .flat_map(|injection| injection.files.clone())
        .collect();
    let files = host_files(&globs, &root);
    let asts = repository_asts(&files, &globs, &root);

    let was = repository_regions(before.injections(), &asts, &root);
    let is = repository_regions(now.injections(), &asts, &root);

    assert!(was.len() > 500, "the repository has regions: {}", was.len());
    assert!(
        now.injections().len() * 2 < before.injections().len(),
        "the entries are consolidated: {} of {}",
        now.injections().len(),
        before.injections().len()
    );
    assert_eq!(is.len(), was.len());
    assert!(is == was, "the regions differ");
}
