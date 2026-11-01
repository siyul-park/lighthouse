//! Lowering a reorder: the blocks out of order are moved, the others stay.

use super::*;

#[test]
fn reorder_moves_the_blocks_out_of_order() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("c"), id("a"), id("b")],
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "fn c() {}\n\nfn a() {}\n\nfn b() {}\n"
    );
}

#[test]
fn reorder_leaves_declarations_it_does_not_list_where_they_are() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("c"), id("a")],
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "fn b() {}\n\nfn c() {}\n\nfn a() {}\n"
    );
}

const GROUPED: &str = "// modes\nconst (\n    a = 1\n    b = 2\n)\n\nfn c() {}\n\nfn d() {}\n";
const GROUPED_SYMBOLS: &[(&str, u32, u32)] = &[("a", 3, 3), ("b", 4, 4), ("c", 7, 7), ("d", 9, 9)];

#[test]
fn reorder_moves_a_parenthesized_declaration_whole_with_its_comment() {
    let case = case(GROUPED, GROUPED_SYMBOLS, vec![]);
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("c"), id("a"), id("b"), id("d")],
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "fn c() {}\n\n// modes\nconst (\n    a = 1\n    b = 2\n)\n\nfn d() {}\n"
    );
}

fn reorder_grouped(order: &[&str]) -> EditOp {
    EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: order.iter().map(|name| id(name)).collect(),
    }
}

#[test]
fn reorder_moves_a_parenthesized_declaration_with_its_block_comment() {
    let text =
        "/* modes\n   of it */\nconst (\n    a = 1\n    b = 2\n)\n\nfn c() {}\n\nfn d() {}\n";
    let case = case(
        text,
        &[("a", 4, 4), ("b", 5, 5), ("c", 8, 8), ("d", 10, 10)],
        vec![],
    );

    assert_eq!(
        case.run(&[reorder_grouped(&["c", "a", "b", "d"])]).unwrap(),
        "fn c() {}\n\n/* modes\n   of it */\nconst (\n    a = 1\n    b = 2\n)\n\nfn d() {}\n"
    );
}

#[test]
fn reorder_declines_specs_of_a_group_ordered_apart() {
    let case = case(GROUPED, GROUPED_SYMBOLS, vec![]);

    let why = case
        .run(&[reorder_grouped(&["b", "c", "a", "d"])])
        .unwrap_err();

    assert!(
        why.contains("`a` is ordered apart from `b`, the first of its grouped declaration"),
        "{why}"
    );
}

#[test]
fn reorder_declines_specs_of_a_group_ordered_against_the_file() {
    let case = case(GROUPED, GROUPED_SYMBOLS, vec![]);

    let why = case
        .run(&[reorder_grouped(&["c", "b", "a", "d"])])
        .unwrap_err();

    assert!(
        why.contains("`a` and `b` of one grouped declaration are ordered against their file order"),
        "{why}"
    );
}

#[test]
fn reorder_declines_a_group_with_a_spec_the_order_does_not_list() {
    let case = case(GROUPED, GROUPED_SYMBOLS, vec![]);

    let why = case.run(&[reorder_grouped(&["c", "a", "d"])]).unwrap_err();

    assert_eq!(
        why,
        "`b` of a grouped declaration is not ordered; it stays in place"
    );
}

#[test]
fn reorder_sorts_each_block_on_its_own() {
    let text = "fn b() {}\n\nimpl X {\n    fn d() {}\n\n    fn c() {}\n}\n\nfn a() {}\n";
    let case = case(
        text,
        &[("b", 1, 1), ("d", 4, 4), ("c", 6, 6), ("a", 9, 9)],
        vec![],
    );
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("a"), id("b"), id("c"), id("d")],
    };

    let sorted = case.run(&[op]).unwrap();

    assert_eq!(
        sorted,
        "impl X {\n    fn c() {}\n\n    fn d() {}\n}\n\nfn a() {}\n\nfn b() {}\n"
    );
}

#[test]
fn reorder_keeps_a_section_comment_with_the_declaration_it_stays_before() {
    let text = "fn a() {}\n\n// DML\n\nfn b() {}\n\nfn c() {}\n";
    let case = case(text, &[("a", 1, 1), ("b", 5, 5), ("c", 7, 7)], vec![]);
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("b"), id("c"), id("a")],
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "// DML\n\nfn b() {}\n\nfn c() {}\n\nfn a() {}\n"
    );
}

#[test]
fn reorder_carries_the_free_comment_above_a_declaration_it_moves() {
    let text = "fn a() {}\n\n// free\n\nfn b() {}\n\nfn c() {}\n";
    let case = case(text, &[("a", 1, 1), ("b", 5, 5), ("c", 7, 7)], vec![]);
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("a"), id("c"), id("b")],
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "fn a() {}\n\nfn c() {}\n\n// free\n\nfn b() {}\n"
    );
}

#[test]
fn reorder_puts_several_blocks_after_a_last_line_without_an_end_of_line() {
    let text = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n\nfn d() {}\n\nfn e() {}";
    let symbols = [
        ("a", 1, 1),
        ("b", 3, 3),
        ("c", 5, 5),
        ("d", 7, 7),
        ("e", 9, 9),
    ];
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: ["c", "d", "e", "a", "b"].map(id).to_vec(),
    };

    assert_eq!(
        case(text, &symbols, vec![]).run(&[op]).unwrap(),
        "fn c() {}\n\nfn d() {}\n\nfn e() {}\n\nfn a() {}\n\nfn b() {}\n"
    );
}

#[test]
fn the_run_that_stays_is_the_longest_in_order() {
    assert_eq!(longest_run(&[]), Vec::<usize>::new());
    assert_eq!(longest_run(&[0, 1, 2]), vec![0, 1, 2]);
    assert_eq!(longest_run(&[3, 2, 1, 0]).len(), 1);
    assert_eq!(longest_run(&[2, 0, 1, 4, 3, 5]).len(), 4);
}

#[test]
fn a_run_of_a_hundred_thousand_ranks_is_found_at_once() {
    let ranks: Vec<usize> = (0..100_000).rev().collect();

    assert_eq!(longest_run(&ranks).len(), 1);
}

#[test]
fn reorder_moves_adjacent_blocks_when_the_last_ends_the_file() {
    let text = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n\nfn d() {}\n\nfn e() {}\n";
    let symbols = [
        ("a", 1, 1),
        ("b", 3, 3),
        ("c", 5, 5),
        ("d", 7, 7),
        ("e", 9, 9),
    ];
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: ["d", "e", "a", "b", "c"].map(id).to_vec(),
    };

    let lowered = case(text, &symbols, vec![]).run(&[op]);

    assert_eq!(
        lowered.unwrap(),
        "fn d() {}\n\nfn e() {}\n\nfn a() {}\n\nfn b() {}\n\nfn c() {}\n"
    );
}

/// A deterministic shuffle (xorshift), so that a failure repeats.
fn shuffled(n: usize, seed: u64) -> Vec<usize> {
    let mut state = seed | 1;
    let mut order: Vec<usize> = (0..n).collect();
    for at in (1..n).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        order.swap(at, usize::try_from(state % (at as u64 + 1)).unwrap());
    }
    order
}

#[test]
fn every_permutation_of_mixed_declarations_lowers_to_disjoint_edits_in_the_target_order() {
    for count in 2..=7 {
        for seed in 1..=150u64 {
            // Declaration `n` is one line when `n + seed` is even, else three.
            let mut text = String::new();
            let mut symbols = Vec::new();
            let mut bodies = Vec::new();
            let mut line = 1u32;
            for n in 0..count {
                let body = if (n as u64 + seed).is_multiple_of(2) {
                    format!("fn d{n}() {{}}\n")
                } else {
                    format!("fn d{n}() {{\n    x\n}}\n")
                };
                let lines = u32::try_from(body.lines().count()).unwrap();
                symbols.push((format!("d{n}"), line, line + lines - 1));
                line += lines + 1;
                text.push_str(&body);
                if n + 1 < count {
                    text.push('\n');
                }
                bodies.push(body);
            }
            let spans: Vec<(&str, u32, u32)> = symbols
                .iter()
                .map(|(n, a, b)| (n.as_str(), *a, *b))
                .collect();
            let target = shuffled(count, seed);
            let op = EditOp::Reorder {
                owner: Owner::File(FILE.into()),
                order: target.iter().map(|n| id(&format!("d{n}"))).collect(),
            };
            let want: Vec<&str> = target.iter().map(|&n| bodies[n].as_str()).collect();

            let got = case(&text, &spans, vec![])
                .run(&[op])
                .unwrap_or_else(|why| {
                    panic!("{count} declarations, seed {seed}, order {target:?}: {why}")
                });

            assert_eq!(got, want.join("\n"), "seed {seed}, order {target:?}");
        }
    }
}
