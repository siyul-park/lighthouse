//! Components, reachability and cycles over a directed graph of dense node indexes.

use lighthouse_model::graph::Graph;

#[test]
fn graph_new_drops_self_loops_and_repeated_edges() {
    let reached = Graph::new(3, [(0, 0), (0, 1), (0, 1), (1, 2)]).reached();

    assert_eq!(reached.ce, [1, 1, 0]);
    assert_eq!(reached.ca, [0, 1, 1]);
}

#[test]
fn graph_reached_counts_what_each_node_reaches_and_is_reached_by() {
    let chain = Graph::new(3, [(0, 1), (1, 2)]).reached();
    assert_eq!(chain.vfo, [2, 1, 0]);
    assert_eq!(chain.vfi, [0, 1, 2]);
    assert_eq!(chain.scc, [2, 1, 0]);
    assert_eq!(chain.sizes, [1, 1, 1]);

    // A component's other members count as reached, the node itself never.
    let cycle = Graph::new(3, [(0, 1), (1, 0), (1, 2)]).reached();
    assert_eq!(cycle.vfo, [2, 2, 0]);
    assert_eq!(cycle.vfi, [1, 1, 2]);
    assert_eq!(cycle.sizes, [1, 2]);
}

#[test]
fn graph_cycles_lists_components_of_two_or_more_in_the_order_of_their_first_nodes() {
    let graph = Graph::new(6, [(2, 3), (3, 2), (0, 1), (1, 0), (4, 4), (4, 5)]);

    assert_eq!(graph.cycles(), [vec![0, 1], vec![2, 3]]);
    assert!(Graph::new(2, [(0, 1)]).cycles().is_empty());
}

#[test]
fn graph_components_numbers_sinks_first_and_survives_a_deep_graph() {
    let (scc, count) = Graph::new(3, [(0, 1), (1, 0), (1, 2)]).components();
    assert_eq!((scc, count), (vec![1, 1, 0], 2));

    let depth = 200_000;
    let path = (0..depth - 1).map(|at| (at, at + 1));
    let (scc, count) = Graph::new(depth as usize, path).components();
    assert_eq!(count, depth as usize);
    assert_eq!(scc[0], depth - 1);
}

#[test]
fn a_node_is_one_higher_than_the_tallest_node_below_it_and_a_cycle_shares_a_height() {
    // 0 -> 1 <-> 2 -> 3; 4 stands alone.
    let graph = Graph::new(5, [(0, 1), (1, 2), (2, 1), (2, 3)]);
    assert_eq!(graph.heights(&[false; 5]), [2, 1, 1, 0, 0]);
}

#[test]
fn a_transparent_node_has_the_height_of_what_it_leads_to() {
    // 0 -> 1 (transparent) -> 2 -> 3
    let graph = Graph::new(4, [(0, 1), (1, 2), (2, 3)]);
    assert_eq!(graph.heights(&[false, true, false, false]), [2, 1, 1, 0]);
}
