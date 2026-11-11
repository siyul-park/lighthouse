//! The `steps_level` and `abstraction_mix` of a function: its top-level
//! statements with what each one does, and how the steps spread over the
//! levels of the call graph. A step's level here is the height of the highest
//! project function it calls; thresholds, neutral calls and the placement of
//! external symbols belong to the rules that read these facts.

use std::collections::BTreeMap;

use lighthouse_model::{
    Span,
    ucm::{EdgeKind, Node, Resolution, Step, Symbol, SymbolId, Target},
};
use serde_json::{Map, Value, json};

use super::Builder;

/// A call of a step: its name, and its height when it is a call of a project
/// function.
struct Called {
    name: String,
    height: Option<u32>,
}

impl Builder<'_> {
    /// One entry per top-level statement of a function with steps; null for
    /// any other symbol:
    /// `{ start, end, kind, operators, indexing, literals, primitive, nested,
    /// guard, neutral, calls, calls_project, calls_external, named_level }`.
    /// `primitive` is `operators + indexing + literals`; `named_level` is the
    /// greatest height among the project functions called in the step, null
    /// when it calls none; `calls` lists `{ name, project, height }`. A step is
    /// `neutral` when it is a guard, or calls nothing and counts nothing.
    pub(super) fn steps_level(&self, symbol: &Symbol) -> Value {
        let Some(summary) = self.project.function(&symbol.id) else {
            return Value::Null;
        };
        if summary.steps.is_empty() {
            return Value::Null;
        }
        let resolved = self.resolved_calls(&symbol.id);
        let entries = summary
            .steps
            .iter()
            .map(|step| self.entry(&resolved, step))
            .collect();
        Value::Array(entries)
    }

    /// `{ height, step_down, steps, unnamed, offsets }` from the `levels` of a
    /// function's steps; null for any other symbol. `height` is the function's height in
    /// the call graph and `step_down` the level of its callees' tallest one;
    /// `steps` counts the steps that are not neutral, `unnamed` those of them
    /// that call no project function, and `offsets` maps the offset of a
    /// step's `named_level` from `step_down` (`"0"`, `"-1"`, ...) to the number
    /// of steps at it.
    pub(super) fn abstraction_mix(&self, symbol: &Symbol, levels: &Value) -> Value {
        let Value::Array(entries) = levels else {
            return Value::Null;
        };
        let height = i64::from(self.indexes.height(&symbol.id).unwrap_or(0));
        let step_down = height - 1;
        let mut offsets: BTreeMap<i64, u64> = BTreeMap::new();
        let (mut steps, mut unnamed) = (0u64, 0u64);
        for entry in entries.iter().filter(|e| e["neutral"] == json!(false)) {
            steps += 1;
            match entry["named_level"].as_i64() {
                Some(level) => *offsets.entry(level - step_down).or_default() += 1,
                None => unnamed += 1,
            }
        }
        let offsets: Map<String, Value> = offsets
            .into_iter()
            .map(|(offset, count)| (offset.to_string(), json!(count)))
            .collect();
        json!({
            "height": height,
            "step_down": step_down,
            "steps": steps,
            "unnamed": unnamed,
            "offsets": offsets,
        })
    }

    fn entry(&self, resolved: &[(&Span, &SymbolId)], step: &Step) -> Value {
        let calls = self.called(resolved, step);
        let primitive = step.operators + step.indexing + step.literals;
        let named_level = calls.iter().filter_map(|c| c.height).max();
        let project = calls.iter().filter(|c| c.height.is_some()).count();
        let neutral = step.guard || (calls.is_empty() && primitive == 0);
        json!({
            "start": step.span.start.line,
            "end": step.span.end.line,
            "kind": step.kind.as_str(),
            "operators": step.operators,
            "indexing": step.indexing,
            "literals": step.literals,
            "primitive": primitive,
            "nested": step.nested,
            "guard": step.guard,
            "neutral": neutral,
            "calls": calls.iter().map(|c| json!({
                "name": c.name,
                "project": c.height.is_some(),
                "height": c.height,
            })).collect::<Vec<_>>(),
            "calls_project": project,
            "calls_external": calls.len() - project,
            "named_level": named_level,
        })
    }

    /// The resolved calls of a function to a project function, by site.
    fn resolved_calls(&self, id: &SymbolId) -> Vec<(&Span, &SymbolId)> {
        self.project
            .edges_from(id)
            .filter(|e| e.kind == EdgeKind::Calls && e.resolution != Resolution::Heuristic)
            .filter_map(|e| match (&e.to, e.site.as_ref()) {
                (Target::Resolved(Node::Symbol(to)), Some(site)) => Some((site, to)),
                _ => None,
            })
            .collect()
    }

    /// The calls of a step, each matched by its site to a resolved call.
    fn called(&self, resolved: &[(&Span, &SymbolId)], step: &Step) -> Vec<Called> {
        step.calls
            .iter()
            .map(|call| Called {
                name: call.name.clone(),
                height: resolved
                    .iter()
                    .find(|(site, _)| site.start == call.site.start)
                    .and_then(|(_, to)| self.indexes.height(to)),
            })
            .collect()
    }
}
