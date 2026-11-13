use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
    time::Instant,
};

use lighthouse_test_support::{bundled, go_root};
use serde_json::{Map, json};

use super::{analysis::judge, *};

const GO: Alphabet = Alphabet {
    tests: false,
    hook: Some("Clone"),
};
const GO_TESTS: Alphabet = Alphabet {
    tests: true,
    hook: Some("Clone"),
};
const RUST: Alphabet = Alphabet {
    tests: false,
    hook: Some("Drop"),
};
const RUST_TESTS: Alphabet = Alphabet {
    tests: true,
    hook: Some("Drop"),
};
const NO_HOOK: Alphabet = Alphabet {
    tests: false,
    hook: None,
};

#[test]
fn shapes_are_counted_once_up_to_renaming() {
    let counts = |language, alphabet| -> Vec<(usize, usize)> {
        (1..=MAX_BOUND)
            .map(|n| {
                let shapes = enumerate(language, alphabet, n).unwrap();
                let arrangements = shapes.iter().map(|s| s.arrangements().len()).sum();
                (shapes.len(), arrangements)
            })
            .collect()
    };

    assert_eq!(
        counts(Language::Go, GO),
        [(9, 9), (110, 198), (1891, 10119)]
    );
    assert_eq!(
        counts(Language::Go, GO_TESTS),
        [(10, 10), (138, 255), (2800, 16320)]
    );
    assert_eq!(
        counts(Language::Rust, RUST),
        [(8, 8), (88, 156), (1464, 7772)]
    );
    assert_eq!(
        counts(Language::Rust, RUST_TESTS),
        [(9, 9), (114, 209), (2262, 13285)]
    );
}

#[test]
fn shapes_come_smallest_first_and_every_owned_declaration_has_one_owner() {
    let shapes = enumerate(Language::Go, GO_TESTS, 3).unwrap();
    let keys: Vec<(usize, usize)> = shapes.iter().map(|s| (s.size(), s.edges.len())).collect();

    assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(shapes.iter().all(|s| s.size() <= 3));
    for shape in &shapes {
        for (index, decl) in shape.decls.iter().enumerate() {
            let owners = shape
                .edges
                .iter()
                .filter(|e| e.from == index && e.relation == Relation::Owner)
                .count();
            assert_eq!(owners, usize::from(decl.kind.owned()), "{shape:?}");
        }
    }
}

#[test]
fn an_owned_declaration_needs_a_type_and_a_type_has_one_of_each() {
    let shapes = enumerate(Language::Go, GO, 3).unwrap();

    for shape in &shapes {
        let owners_of = |kind| -> Vec<usize> {
            let mut owners: Vec<usize> = shape
                .edges
                .iter()
                .filter(|e| e.relation == Relation::Owner && shape.decls[e.from].kind == kind)
                .map(|e| e.to)
                .collect();
            owners.sort_unstable();
            owners
        };
        for kind in [Kind::Constructor, Kind::Option, Kind::Hook] {
            let owners = owners_of(kind);
            let mut once = owners.clone();
            once.dedup();
            assert_eq!(owners, once, "{kind:?} twice on one type: {shape:?}");
        }
        for call in shape.edges.iter().filter(|e| e.relation == Relation::Calls) {
            if shape.decls[call.from].kind == Kind::Method
                && shape.decls[call.to].kind == Kind::Method
            {
                let owner = |m: usize| {
                    shape
                        .edges
                        .iter()
                        .find(|e| e.from == m && e.relation == Relation::Owner)
                };
                assert_eq!(owner(call.from).map(|e| e.to), owner(call.to).map(|e| e.to));
            }
        }
    }
    assert!(
        !shapes
            .iter()
            .any(|s| s.decls.len() == 1 && s.decls[0].kind.owned())
    );
}

#[test]
fn the_kinds_of_a_language_are_its_own() {
    let kinds = |language, alphabet| -> Vec<Kind> {
        let mut kinds: Vec<Kind> = enumerate(language, alphabet, 2)
            .unwrap()
            .iter()
            .flat_map(|s| s.decls.iter().map(|d| d.kind))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    };

    assert!(kinds(Language::Go, GO).contains(&Kind::Init));
    assert!(kinds(Language::Go, GO).contains(&Kind::Option));
    assert!(!kinds(Language::Rust, RUST).contains(&Kind::Init));
    assert!(!kinds(Language::Rust, RUST).contains(&Kind::Option));
    assert!(kinds(Language::Go, GO).contains(&Kind::Hook));
    assert!(!kinds(Language::Go, NO_HOOK).contains(&Kind::Hook));
}

#[test]
fn only_what_a_test_can_call_is_tested() {
    let shapes = enumerate(Language::Go, GO_TESTS, 3).unwrap();
    let tested: Vec<&Decl> = shapes
        .iter()
        .flat_map(|s| &s.decls)
        .filter(|d| d.tested)
        .collect();

    assert!(!tested.is_empty());
    assert!(tested.iter().all(|d| d.visibility == Visibility::Public));
    assert!(
        tested
            .iter()
            .all(|d| matches!(d.kind, Kind::Func | Kind::Method))
    );
    assert!(
        !enumerate(Language::Go, GO, 3)
            .unwrap()
            .iter()
            .flat_map(|s| &s.decls)
            .any(|d| d.tested)
    );
}

#[test]
fn arrangements_are_the_orders_that_render_differently() {
    let two = |a: Kind, b: Kind| Shape {
        decls: vec![decl(a, Visibility::Public), decl(b, Visibility::Public)],
        edges: Vec::new(),
    };

    assert_eq!(two(Kind::Func, Kind::Func).arrangements().len(), 1);
    assert_eq!(two(Kind::Func, Kind::Const).arrangements().len(), 2);
}

#[test]
fn tests_are_ordered_too() {
    let tested = Decl {
        tested: true,
        ..decl(Kind::Func, Visibility::Public)
    };
    let shape = Shape {
        decls: vec![tested, tested],
        edges: Vec::new(),
    };

    assert_eq!(shape.size(), 2, "the tests do not count");
    assert_eq!(
        shape.arrangements().len(),
        2,
        "tests in the order of the sources, or not"
    );
    let other = Shape {
        decls: vec![tested, decl(Kind::Func, Visibility::Public)],
        edges: Vec::new(),
    };
    assert_eq!(other.arrangements().len(), 2);
}

#[test]
fn a_bound_above_the_largest_is_incomplete() {
    assert_eq!(
        enumerate(Language::Go, GO, MAX_BOUND + 1),
        Err(Incomplete::Bound(MAX_BOUND + 1))
    );
}

#[test]
fn language_of_an_id() {
    assert_eq!(Language::of("go"), Ok(Language::Go));
    assert_eq!(
        Language::of("python"),
        Err(Incomplete::NoRenderer("python".to_owned()))
    );
}

#[test]
fn language_id() {
    assert_eq!(Language::Go.id(), "go");
    assert_eq!(Language::of(Language::Rust.id()), Ok(Language::Rust));
}

#[test]
fn setting_new() {
    let provenance = Provenance {
        renderer: "r".to_owned(),
        provider: "p@1".to_owned(),
        tool: "t".to_owned(),
        injected: String::new(),
    };
    let setting = Setting::new(Language::Go, GO, 2, provenance);

    assert_eq!(
        (setting.language.as_str(), setting.bound, setting.tests),
        ("go", 2, false)
    );
    assert_eq!(setting.hook.as_deref(), Some("Clone"));
}

#[test]
fn verdict_incomplete() {
    let verdict = Verdict::incomplete(vec!["why".to_owned()]);

    assert!(verdict.contradictions.is_empty() && verdict.single.is_empty());
    assert_eq!(verdict.basis, Basis::Incomplete(vec!["why".to_owned()]));
}

#[test]
fn scratch_digest() {
    let shapes = enumerate(Language::Rust, RUST, 2).unwrap();
    let digest = Scratch::digest(Language::Rust, RUST, &shapes);

    assert_eq!(digest, Scratch::digest(Language::Rust, RUST, &shapes));
    assert_ne!(digest, Scratch::digest(Language::Rust, RUST, &shapes[1..]));
    let go = enumerate(Language::Go, GO, 2).unwrap();
    assert_ne!(digest, Scratch::digest(Language::Go, GO, &go));
    assert_ne!(
        Scratch::digest(Language::Go, GO, &go),
        Scratch::digest(
            Language::Go,
            NO_HOOK,
            &enumerate(Language::Go, NO_HOOK, 2).unwrap()
        )
    );
}

#[test]
fn a_decision_unsatisfiable_by_itself_hides_every_pair_that_holds_it() {
    let (d, e) = (assumption("D"), assumption("E"));
    let setting = Setting {
        language: "rust".to_owned(),
        bound: 2,
        tests: false,
        hook: Some("Drop".to_owned()),
        renderer: String::new(),
        provider: String::new(),
        tool: String::new(),
        injected: String::new(),
    };
    let shapes = enumerate(Language::Rust, RUST, 2).unwrap();
    let mut base = 0;
    let mut pairs = Vec::new();
    for shape in &shapes {
        let count = shape.arrangements().len();
        if shape.size() == 2 && count == 2 {
            pairs.push(base);
        }
        base += count;
    }
    // D and E flag one arrangement each of an early shape; D flags both
    // arrangements of a later one.
    let early = pairs[0];
    let late = pairs[1];
    let together = BTreeMap::from([
        (d.clone(), BTreeSet::from([early, late, late + 1])),
        (e.clone(), BTreeSet::from([early + 1])),
    ]);
    let pair_only = BTreeMap::from([
        (d.clone(), BTreeSet::from([early])),
        (e.clone(), BTreeSet::from([early + 1])),
    ]);

    let alone = judge(&setting, &together);
    let both = judge(&setting, &pair_only);

    assert!(
        alone.contradictions.is_empty(),
        "{:?}",
        alone.contradictions
    );
    assert_eq!(
        alone
            .single
            .iter()
            .map(|s| s.decision.as_str())
            .collect::<Vec<_>>(),
        ["D"]
    );
    assert_eq!(both.contradictions.len(), 1);
    assert_eq!(both.contradictions[0].decisions, ["D", "E"]);
    assert!(both.single.is_empty());
}

#[test]
fn kind_owned() {
    assert!(Kind::Constructor.owned() && Kind::Hook.owned() && Kind::Method.owned());
    assert!(!Kind::Func.owned() && !Kind::Type.owned());
}

#[test]
fn kind_acts() {
    assert!(Kind::Func.acts() && Kind::Init.acts() && Kind::Constructor.acts());
    assert!(!Kind::Hook.acts() && !Kind::Type.acts());
}

#[test]
fn language_groups() {
    assert!(Language::Go.groups().contains(&"init"));
    assert!(!Language::Rust.groups().contains(&"init"));
    assert!(Language::Rust.groups().contains(&"method"));
}

#[test]
fn the_alphabet_follows_the_decisions_of_the_cell() {
    let groups = bundled().decision("design/declaration-groups").unwrap();
    let layout = bundled().decision("testing/file-layout").unwrap();
    let none = Map::new();

    let go = Alphabet::of(Language::Go, &[(groups, &none)]).unwrap();
    assert_eq!(
        go,
        Alphabet {
            tests: false,
            hook: Some("Clone")
        }
    );
    let go_tests = Alphabet::of(Language::Go, &[(groups, &none), (layout, &none)]).unwrap();
    assert!(go_tests.tests);
    let rust = Alphabet::of(Language::Rust, &[(groups, &none)]).unwrap();
    assert_eq!(
        rust,
        Alphabet {
            tests: false,
            hook: Some("Drop")
        }
    );
}

#[test]
fn a_group_or_hook_the_alphabet_cannot_produce_makes_the_cell_incomplete() {
    let groups = bundled().decision("design/declaration-groups").unwrap();
    let options = |value: serde_json::Value| value.as_object().cloned().unwrap();
    let missing = |group: &str, language: &str| Incomplete::Alphabet {
        decision: "design/declaration-groups".to_owned(),
        language: language.to_owned(),
        group: group.to_owned(),
    };

    assert_eq!(
        Alphabet::of(
            Language::Rust,
            &[(groups, &options(json!({ "groups": ["init"] })))]
        ),
        Err(missing("init", "rust"))
    );
    assert_eq!(
        Alphabet::of(
            Language::Go,
            &[(groups, &options(json!({ "hooks": ["MarshalJSON"] })))]
        ),
        Err(missing("hooks", "go"))
    );
    let hook = Alphabet::of(
        Language::Go,
        &[(
            groups,
            &options(json!({ "hooks": ["MarshalJSON", "Error"] })),
        )],
    );
    assert_eq!(hook.unwrap().hook, Some("Error"));
    let no_hook = Alphabet::of(Language::Go, &[(groups, &options(json!({ "hooks": [] })))]);
    assert_eq!(no_hook.unwrap().hook, None);
    assert!(matches!(
        Alphabet::of(Language::Go, &[(groups, &options(json!({ "nope": 1 })))]),
        Err(Incomplete::Options { .. })
    ));
}

#[test]
fn constants_and_variables_are_always_separate_declarations() {
    let shapes = enumerate(Language::Go, GO, 2).unwrap();
    let scratch = Scratch::render(Language::Go, GO, &shapes).unwrap();

    for unit in scratch.units() {
        let text = read(scratch.root(), &unit.files[0]);
        assert!(
            !text.contains("const (") && !text.contains("var ("),
            "{text}"
        );
    }
}

#[test]
fn a_constructor_and_an_option_builder_are_named_after_their_type() {
    let shape = Shape {
        decls: vec![
            decl(Kind::Type, Visibility::Private),
            decl(Kind::Constructor, Visibility::Public),
            decl(Kind::Option, Visibility::Private),
        ],
        edges: vec![
            Edge {
                from: 1,
                to: 0,
                relation: Relation::Owner,
            },
            Edge {
                from: 2,
                to: 0,
                relation: Relation::Owner,
            },
        ],
    };
    let scratch = Scratch::render(Language::Go, GO, &[shape]).unwrap();
    let texts: Vec<String> = scratch
        .units()
        .iter()
        .map(|u| read(scratch.root(), &u.files[0]))
        .collect();

    assert!(
        texts
            .iter()
            .all(|t| t.contains("func NewT0() *t0 {\n\treturn &t0{}\n}")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .all(|t| t.contains("func withT0() func(*t0) {\n\treturn func(*t0) {}\n}")),
        "{texts:?}"
    );
}

#[test]
fn arrangements_merged_under_renaming_differ_only_in_the_digits_of_names() {
    let digits = |text: &str| text.replace(|c: char| c.is_ascii_digit(), "#");
    for shape in enumerate(Language::Go, GO_TESTS, 3).unwrap() {
        let mut classes: BTreeMap<_, Vec<String>> = BTreeMap::new();
        for source in permutations(shape.decls.len()) {
            let arrangement = Arrangement {
                tests: source
                    .iter()
                    .copied()
                    .filter(|&i| shape.decls[i].tested)
                    .collect(),
                source,
            };
            let files = text::files(Language::Go, GO_TESTS, &shape, &arrangement, "p0");
            let text = files
                .iter()
                .map(|(_, body)| body.as_str())
                .collect::<String>();
            classes
                .entry(shape.layout(&arrangement))
                .or_default()
                .push(digits(&text));
        }
        for texts in classes.values() {
            assert!(
                texts.windows(2).all(|pair| pair[0] == pair[1]),
                "{shape:?}\n{texts:#?}"
            );
        }
    }
}

#[test]
fn edge_sets_counted_by_isomorphism_agree_with_the_enumeration() {
    for language in [Language::Go, Language::Rust] {
        let alphabet = if language == Language::Go {
            GO_TESTS
        } else {
            RUST_TESTS
        };
        let labels = alphabet.labels(language);
        let shapes = enumerate(language, alphabet, 3).unwrap();
        for decls in multisets(&labels) {
            let expected = isomorphism_classes(&decls);
            let counted = shapes.iter().filter(|s| s.decls == decls).count();
            assert_eq!(counted, expected, "{language:?} {decls:?}");
        }
    }
}

#[test]
fn every_rendered_go_package_passes_vet() {
    let Some(root) = go_root() else { return };
    let shapes = enumerate(Language::Go, GO_TESTS, 2).unwrap();
    let scratch = Scratch::render(Language::Go, GO_TESTS, &shapes).unwrap();

    assert!(scratch.units().len() > shapes.len());
    run(scratch.root(), &root.join("bin/go"), &["vet", "./..."]);
}

#[test]
#[ignore = "slow: run by `make decisions` in release"]
fn every_rendered_go_package_of_three_declarations_passes_vet() {
    let Some(root) = go_root() else { return };
    let shapes: Vec<Shape> = enumerate(Language::Go, GO_TESTS, 3)
        .unwrap()
        .into_iter()
        .filter(|s| s.size() == 3)
        .collect();
    let scratch = Scratch::render(Language::Go, GO_TESTS, &shapes).unwrap();

    run(scratch.root(), &root.join("bin/go"), &["vet", "./..."]);
}

#[test]
fn every_rendered_rust_crate_type_checks_with_the_toolchain_of_cargo() {
    let shapes = enumerate(Language::Rust, RUST_TESTS, 2).unwrap();
    let scratch = Scratch::render(Language::Rust, RUST_TESTS, &shapes).unwrap();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    run(
        scratch.root(),
        Path::new(&cargo),
        &["check", "--tests", "--offline", "--quiet"],
    );
}

#[test]
#[ignore = "slow: run by `make decisions` in release"]
fn every_rendered_rust_crate_of_three_declarations_type_checks() {
    let shapes = enumerate(Language::Rust, RUST_TESTS, 3).unwrap();
    let scratch = Scratch::render(Language::Rust, RUST_TESTS, &shapes).unwrap();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    run(
        scratch.root(),
        Path::new(&cargo),
        &["check", "--tests", "--offline", "--quiet"],
    );
}

#[test]
fn a_package_holds_one_arrangement_and_names_its_files() {
    let shapes = enumerate(Language::Go, GO_TESTS, 2).unwrap();
    let scratch = Scratch::render(Language::Go, GO_TESTS, &shapes).unwrap();
    let unit = scratch.units().iter().find(|u| u.files.len() == 2).unwrap();

    assert_eq!(
        unit.files[0],
        Path::new(&unit.package).join(format!("{}.go", unit.package))
    );
    assert_eq!(
        unit.files[1],
        Path::new(&unit.package).join(format!("{}_test.go", unit.package))
    );
    assert!(
        read(scratch.root(), &unit.files[0]).starts_with(&format!("package {}\n", unit.package))
    );
    assert!(
        shapes[unit.shape]
            .arrangements()
            .contains(&unit.arrangement)
    );
}

#[test]
fn the_scratch_project_is_removed_when_dropped() {
    let shapes = enumerate(Language::Rust, RUST, 2).unwrap();
    let scratch = Scratch::render(Language::Rust, RUST, &shapes).unwrap();
    let root = scratch.root().to_owned();

    assert!(root.join("Cargo.toml").is_file() && root.join("src/lib.rs").is_file());
    drop(scratch);
    assert!(!root.exists());
}

/// How long the bound three takes to enumerate and render, which the oracle's
/// cost rests on: run with `--ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn the_cost_of_the_bound_three() {
    for (language, alphabet) in [(Language::Go, GO_TESTS), (Language::Rust, RUST_TESTS)] {
        let start = Instant::now();
        let shapes = enumerate(language, alphabet, 3).unwrap();
        let enumerated = start.elapsed();
        let scratch = Scratch::render(language, alphabet, &shapes).unwrap();
        println!(
            "{language:?}: {} shapes in {enumerated:?}, {} packages rendered in {:?}",
            shapes.len(),
            scratch.units().len(),
            start.elapsed() - enumerated
        );
    }
}

/// The assumption of a decision with this uid.
fn assumption(uid: &str) -> lighthouse_logic::Assumption {
    lighthouse_logic::Assumption {
        uid: uid.to_owned(),
        digest: "1".to_owned(),
    }
}

/// A declaration of this kind, not tested.
fn decl(kind: Kind, visibility: Visibility) -> Decl {
    Decl {
        kind,
        visibility,
        tested: false,
    }
}

/// Every multiset of the labels of up to three declarations, each in label
/// order.
fn multisets(labels: &[Decl]) -> Vec<Vec<Decl>> {
    let mut all = Vec::new();
    for (i, a) in labels.iter().enumerate() {
        all.push(vec![*a]);
        for (j, b) in labels.iter().enumerate().skip(i) {
            all.push(vec![*a, *b]);
            for c in labels.iter().skip(j) {
                all.push(vec![*a, *b, *c]);
            }
        }
    }
    all
}

/// The number of classes of the edge sets over `decls` that no renaming of
/// equal declarations maps onto each other, found by trying every renaming
/// against every representative.
fn isomorphism_classes(decls: &[Decl]) -> usize {
    let mut representatives: Vec<Vec<Edge>> = Vec::new();
    for mut edges in build::edge_sets(decls) {
        edges.sort();
        let known = representatives
            .iter()
            .any(|known| isomorphic(decls, known, &edges));
        if !known {
            representatives.push(edges);
        }
    }
    representatives.len()
}

/// Whether some permutation of the declarations that keeps every label maps
/// the edges `a` onto the edges `b`.
fn isomorphic(decls: &[Decl], a: &[Edge], b: &[Edge]) -> bool {
    permutations(decls.len()).iter().any(|p| {
        (0..decls.len()).all(|i| decls[p[i]] == decls[i]) && {
            let mut mapped: Vec<Edge> = a
                .iter()
                .map(|e| Edge {
                    from: p[e.from],
                    to: p[e.to],
                    relation: e.relation,
                })
                .collect();
            mapped.sort();
            mapped == b
        }
    })
}

fn read(root: &Path, file: &Path) -> String {
    std::fs::read_to_string(root.join(file)).unwrap()
}

/// Runs `program` in `dir` and fails with its output unless it succeeds.
fn run(dir: &Path, program: &Path, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{program:?} {args:?}\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
