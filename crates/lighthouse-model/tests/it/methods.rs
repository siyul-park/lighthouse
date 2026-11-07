//! Which type satisfies which interface of a language, settled across its
//! units from the method sets the providers state.

use std::sync::Arc;

use lighthouse_graph::{Durability, Graph, Options};
use lighthouse_model::ucm::{
    At, IMPLEMENTS, MEMBERS, Methods, PAYLOAD, Payload, Pending, Provided, Required, SETTLED,
    SymbolId,
};

fn id(text: &str) -> SymbolId {
    SymbolId::parse(text).unwrap()
}

fn at(file: &str, line: u32) -> At {
    At {
        file: file.to_owned(),
        line,
        col: 1,
    }
}

fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|key| (*key).to_owned()).collect()
}

fn provided(symbol: &str, at: At, list: &[&str]) -> Provided {
    Provided {
        symbol: id(symbol),
        at,
        keys: keys(list),
    }
}

fn required(symbol: &str, at: At, list: &[&str], edges: bool) -> Required {
    Required {
        symbol: id(symbol),
        at,
        keys: keys(list),
        edges,
    }
}

fn set(graph: &mut Graph, units: &[(&str, Methods)]) {
    let ids = units.iter().map(|(unit, _)| (*unit).to_owned()).collect();
    graph
        .set_input(&MEMBERS, &"go".to_owned(), ids, Durability::Volatile)
        .unwrap();
    for (unit, methods) in units {
        let payload = Payload {
            methods: methods.clone(),
            ..Payload::default()
        };
        let at = ("go".to_owned(), (*unit).to_owned());
        graph
            .set_input(&PAYLOAD, &at, payload, Durability::Volatile)
            .unwrap();
    }
}

fn settled(graph: &Graph, unit: &str) -> Arc<lighthouse_model::ucm::Settled> {
    graph
        .query(&SETTLED, &("go".to_owned(), unit.to_owned()))
        .unwrap()
}

/// `api` declares the interfaces, `impl` the types.
fn fixture() -> Vec<(&'static str, Methods)> {
    let api = Methods {
        interfaces: vec![
            required(
                "api::Both#interface",
                at("api/api.go", 3),
                &["A", "B"],
                true,
            ),
            required("api::One#interface", at("api/api.go", 9), &["A"], false),
            required(
                "api::Pair#interface",
                at("api/api.go", 12),
                &["A", "B"],
                true,
            ),
        ],
        ..Methods::default()
    };
    let implementation = Methods {
        types: vec![
            provided("impl::Z#type", at("impl/impl.go", 20), &["A", "B"]),
            provided("impl::Y#type", at("impl/impl.go", 5), &["A", "B", "C"]),
            provided("impl::None#type", at("impl/impl.go", 30), &["C"]),
        ],
        pending: vec![Pending {
            method: id("impl::None::C#method"),
            key: "C".to_owned(),
            carriers: vec![id("impl::None#type")],
        }],
        ..Methods::default()
    };
    vec![("api", api), ("impl", implementation)]
}

#[test]
fn a_type_satisfies_the_interfaces_of_another_unit_in_the_order_of_the_declarations() {
    let mut graph = Graph::in_memory(Options::default()).unwrap();
    set(&mut graph, &fixture());

    let got = settled(&graph, "impl");

    assert_eq!(
        got.edges,
        [
            (id("impl::Y#type"), id("api::Both#interface")),
            (id("impl::Y#type"), id("api::Pair#interface")),
            (id("impl::Z#type"), id("api::Both#interface")),
            (id("impl::Z#type"), id("api::Pair#interface")),
        ],
        "no edge to an interface of one method, and none from a type that lacks a method"
    );
    assert!(settled(&graph, "api").edges.is_empty());
}

#[test]
fn a_pending_method_is_settled_when_a_carrier_provides_what_an_interface_requires() {
    let mut graph = Graph::in_memory(Options::default()).unwrap();
    let mut units = fixture();
    units[1].1.pending = vec![
        Pending {
            method: id("impl::Y::A#method"),
            key: "A".to_owned(),
            carriers: vec![id("impl::Z#type"), id("impl::Y#type")],
        },
        Pending {
            method: id("impl::None::C#method"),
            key: "C".to_owned(),
            carriers: vec![id("impl::None#type")],
        },
        Pending {
            method: id("impl::W::B#method"),
            key: "B".to_owned(),
            carriers: vec![id("impl::Nowhere#type")],
        },
    ];
    set(&mut graph, &units);

    let got = settled(&graph, "impl");

    assert_eq!(
        got.methods.iter().collect::<Vec<_>>(),
        [&id("impl::Y::A#method")],
        "C is required by no interface, and B has no carrier that provides what Both requires"
    );
}

#[test]
fn an_edit_that_leaves_the_method_sets_as_they_were_leaves_what_read_them_green() {
    let mut graph = Graph::in_memory(Options::default()).unwrap();
    set(&mut graph, &fixture());
    let before = graph.query(&IMPLEMENTS, &"go".to_owned()).unwrap();
    let unit = settled(&graph, "impl");

    // The unit's answer moved elsewhere (a body), its method sets did not.
    let mut again = fixture();
    set(&mut graph, &again);
    let payload = Payload {
        sources: b"[]".to_vec(),
        methods: again.remove(1).1,
        ..Payload::default()
    };
    let at = ("go".to_owned(), "impl".to_owned());
    graph
        .set_input(&PAYLOAD, &at, payload, Durability::Volatile)
        .unwrap();

    assert!(Arc::ptr_eq(
        &before,
        &graph.query(&IMPLEMENTS, &"go".to_owned()).unwrap()
    ));
    assert!(Arc::ptr_eq(&unit, &settled(&graph, "impl")));
}

#[test]
fn a_method_set_that_moved_moves_what_it_decided() {
    let mut graph = Graph::in_memory(Options::default()).unwrap();
    set(&mut graph, &fixture());
    assert_eq!(settled(&graph, "impl").edges.len(), 4);

    let mut units = fixture();
    units[0].1.interfaces[0].keys = keys(&["A", "B", "D"]);
    set(&mut graph, &units);

    let got = settled(&graph, "impl");
    assert_eq!(
        got.edges,
        [
            (id("impl::Y#type"), id("api::Pair#interface")),
            (id("impl::Z#type"), id("api::Pair#interface")),
        ]
    );
}
