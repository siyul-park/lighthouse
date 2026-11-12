//! The levels of a synthetic project: the longest paths adjusted so that what
//! one step uses together sits on one level.

use lighthouse_checks::level::{Edges, Externals, LevelOptions, Levels, Slot, neutral_set};
use lighthouse_model::{
    Position, Span,
    ucm::{EdgeKind, Step, StepCall, StepKind, Symbol},
};

use crate::support::{World, at};

fn span(line: u32) -> Span {
    Span {
        start: Position { line, col: 1 },
        end: Position {
            line: line + 1,
            col: 1,
        },
    }
}

fn step(line: u32, calls: &[(&str, u32)], operators: u32) -> Step {
    Step {
        span: span(line),
        kind: StepKind::Call,
        operators,
        indexing: 0,
        literals: 0,
        calls: calls
            .iter()
            .map(|(name, at_line)| StepCall {
                name: (*name).to_owned(),
                site: at(*at_line),
            })
            .collect(),
        nested: 0,
        guard: false,
    }
}

fn function(w: &mut World, name: &str, steps: Vec<Step>) -> Symbol {
    let symbol = w.func("m", name, "m/a.ucm");
    w.steps(&symbol, steps);
    symbol
}

fn call(w: &mut World, from: &Symbol, to: &Symbol, line: u32) {
    w.edge_at(EdgeKind::Calls, from, to, line);
}

fn options() -> LevelOptions {
    LevelOptions {
        min_ops: 2,
        neutral_calls: vec!["log.*".to_owned()],
        ..LevelOptions::default()
    }
}

#[test]
fn the_start_is_the_height_plus_one_and_a_chain_stays_there() {
    let mut w = World::default();
    let h = function(&mut w, "h", vec![step(1, &[], 2)]);
    let g = function(&mut w, "g", vec![step(1, &[("h", 1)], 0)]);
    let f = function(&mut w, "f", vec![step(1, &[("g", 1)], 0)]);
    call(&mut w, &g, &h, 1);
    call(&mut w, &f, &g, 1);
    let levels = Levels::of(&w.project(), &options()).unwrap();
    for (symbol, level) in [(&h, 1), (&g, 2), (&f, 3)] {
        assert_eq!(levels.start(&symbol.id), Some(level));
        assert_eq!(levels.level(&symbol.id), Some(level));
    }
    assert_eq!(levels.slots(&h.id), [Slot::Atom]);
}

/// `f` calls `g` (2 high, over `k`), a neutral-bodied `h` and the external
/// `x.Put` in one step.
fn together() -> (World, [Symbol; 5]) {
    let mut w = World::default();
    let k = function(&mut w, "k", vec![step(1, &[], 2)]);
    let g = function(&mut w, "g", vec![step(1, &[("k", 1)], 0)]);
    let h = function(&mut w, "h", vec![step(1, &[], 0)]);
    let f = function(
        &mut w,
        "f",
        vec![step(1, &[("g", 1), ("h", 2), ("x.Put", 3)], 0)],
    );
    call(&mut w, &g, &k, 1);
    call(&mut w, &f, &g, 1);
    call(&mut w, &f, &h, 2);
    (w, [k, g, h, f.clone(), f])
}

fn external(levels: &Levels, name: &str) -> i64 {
    levels
        .externals()
        .find(|(_, n, _, _)| *n == name)
        .map(|(_, _, _, level)| level)
        .unwrap()
}

#[test]
fn what_a_step_calls_together_is_pulled_to_one_level() {
    let (w, [k, g, h, f, _]) = together();
    let levels = Levels::of(&w.project(), &options()).unwrap();
    assert_eq!(levels.start(&h.id), Some(1));
    assert_eq!(
        levels.level(&h.id),
        Some(2),
        "h rises to g, free of any caller"
    );
    assert_eq!(levels.level(&g.id), Some(2));
    assert_eq!(levels.level(&f.id), Some(3));
    assert_eq!(levels.level(&k.id), Some(1));
    assert_eq!(
        external(&levels, "x.Put"),
        2,
        "the external rises with them"
    );
    assert!(levels.stats.cost < levels.stats.start_cost);
}

#[test]
fn the_top_callee_variant_and_frozen_externals_change_the_arcs() {
    let (w, [_, _, h, _, _]) = together();
    let top = Levels::of(
        &w.project(),
        &LevelOptions {
            edges: Edges::Top,
            ..options()
        },
    )
    .unwrap();
    assert_eq!(top.level(&h.id), Some(1), "only g pulls in the top variant");
    let fixed = Levels::of(
        &w.project(),
        &LevelOptions {
            externals: Externals::Fixed,
            ..options()
        },
    )
    .unwrap();
    assert_eq!(external(&fixed, "x.Put"), 1);
    assert_eq!(fixed.level(&h.id), Some(2));
}

#[test]
fn an_external_symbol_sits_where_its_callers_put_it() {
    let mut w = World::default();
    let low = function(&mut w, "low", vec![step(1, &[("b.WriteByte", 1)], 0)]);
    let mid = function(
        &mut w,
        "mid",
        vec![step(1, &[("low", 1), ("j.Marshal", 2)], 0)],
    );
    let top = function(
        &mut w,
        "top",
        vec![
            step(1, &[("mid", 1)], 0),
            step(3, &[("j.Marshal", 3)], 0),
            step(5, &[("j.Marshal", 5)], 0),
        ],
    );
    call(&mut w, &mid, &low, 1);
    call(&mut w, &top, &mid, 1);
    let levels = Levels::of(&w.project(), &options()).unwrap();
    // The floor of an external is 1, so a function that calls one wants to
    // stand at 2: the whole chain rises by one, which costs nothing between its
    // functions.
    assert_eq!(levels.start(&top.id), Some(3));
    assert_eq!(levels.level(&low.id), Some(2));
    assert_eq!(levels.level(&top.id), Some(4));
    assert_eq!(
        external(&levels, "b.WriteByte"),
        1,
        "the floor, under its only caller"
    );
    assert_eq!(
        external(&levels, "j.Marshal"),
        3,
        "level with the callee of the top steps"
    );
}

#[test]
fn a_neutral_call_is_no_edge_and_a_step_of_it_is_neutral() {
    let mut w = World::default();
    let f = function(&mut w, "f", vec![step(1, &[("log.Println", 1)], 0)]);
    let levels = Levels::of(&w.project(), &options()).unwrap();
    assert_eq!(levels.slots(&f.id), [Slot::Neutral]);
    assert_eq!(levels.stats.externals, 0);
}

#[test]
fn a_guard_is_neutral_and_few_primitives_are_neutral() {
    let mut w = World::default();
    let mut guard = step(1, &[("x.Fail", 1)], 5);
    guard.guard = true;
    let f = function(&mut w, "f", vec![guard, step(2, &[], 1), step(3, &[], 2)]);
    let levels = Levels::of(&w.project(), &options()).unwrap();
    assert_eq!(
        levels.slots(&f.id),
        [Slot::Neutral, Slot::Neutral, Slot::Atom]
    );
}

#[test]
fn a_glob_stands_for_any_run_of_characters() {
    let set = |patterns: &[&str]| {
        neutral_set(&LevelOptions {
            neutral_calls: patterns.iter().map(|p| (*p).to_owned()).collect(),
            ..LevelOptions::default()
        })
        .unwrap()
    };
    let set = set(&[
        "log.*",
        "log/slog.*",
        "testing.T.Log*",
        "*!",
        ".clone",
        "fmt.Errorf",
    ]);
    for name in [
        "log.Printf",
        "log/slog.Info",
        "testing.T.Logf",
        "tracing::info!",
        ".clone",
    ] {
        assert!(set.is_match(name), "{name}");
    }
    for name in ["slog.Info", "fmt.Errorf2", ".cloned"] {
        assert!(!set.is_match(name), "{name}");
    }
}

#[test]
fn an_invalid_glob_is_an_error() {
    let options = LevelOptions {
        neutral_calls: vec!["[".to_owned()],
        ..LevelOptions::default()
    };
    assert!(neutral_set(&options).is_err());
}

#[test]
fn levels_step_level_is_the_highest_named_operation_and_none_for_neutral() {
    let mut w = World::default();
    let k = function(&mut w, "k", vec![step(1, &[], 2)]);
    let f = function(
        &mut w,
        "f",
        vec![step(1, &[("k", 1)], 0), step(2, &[], 3), step(3, &[], 0)],
    );
    call(&mut w, &f, &k, 1);
    let levels = Levels::of(&w.project(), &options()).unwrap();
    let found: Vec<_> = levels
        .slots(&f.id)
        .iter()
        .map(|slot| levels.step_level(slot, true))
        .collect();
    assert_eq!(found, [Some(1), Some(0), None]);
}
