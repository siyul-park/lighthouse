mod cognitive;
mod fan;

use std::{collections::BTreeMap, sync::LazyLock};

use lighthouse_model::{FlowKind, FunctionSummary, Project, Symbol, SymbolId};
use lighthouse_plugin::{Analyzer, AnalyzerManifest, Ctx, Error, Plugin, PluginManifest, Scope};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub use fan::Fan;

/// Plugin id.
const ID: &str = "metrics";
/// Analyzer id of the per-function [`Size`] fact.
pub const SIZE: &str = "metrics/size";
/// Analyzer id of the per-function cyclomatic complexity fact (`u32`).
pub const CYCLOMATIC: &str = "metrics/cyclomatic";
/// Analyzer id of the per-function cognitive complexity fact (`u32`).
pub const COGNITIVE: &str = "metrics/cognitive";
/// Analyzer id of the per-function deepest-nesting fact (`u32`).
pub const NESTING: &str = "metrics/nesting";
/// Analyzer id of the per-function [`Fan`] fact.
pub const FAN: &str = "metrics/fan";

/// Deepest nesting of a flat dispatch's arms.
const FLAT_DISPATCH_NESTING: u32 = 2;

/// A function's value of one metric; every metric fact is a list of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measured<T> {
    pub symbol: SymbolId,
    pub value: T,
}

/// Statement and line count of a function; `lines` is inclusive of both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub statements: u32,
    pub lines: u32,
}

/// The plugin providing the `metrics/*` analyzers; stateless.
pub struct Metrics;

impl Plugin for Metrics {
    /// Identifies the plugin as `metrics` at this crate's version.
    fn manifest(&self) -> &PluginManifest {
        static MANIFEST: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        &MANIFEST
    }

    /// One per-function analyzer per metric constant, each file-scoped.
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        vec![
            Box::new(PerFunction::new(SIZE, size)),
            Box::new(PerFunction::new(CYCLOMATIC, cyclomatic)),
            Box::new(PerFunction::new(COGNITIVE, cognitive::score)),
            Box::new(PerFunction::new(NESTING, nesting)),
            Box::new(PerFunction::new(FAN, |project, symbol, _| {
                fan::measure(project, symbol)
            })),
        ]
    }
}

type Measure<T> = fn(&Project, &Symbol, &FunctionSummary) -> T;

/// Measures every function declared in the focused file.
struct PerFunction<T> {
    manifest: AnalyzerManifest,
    measure: Measure<T>,
}

impl<T> PerFunction<T> {
    fn new(id: &'static str, measure: Measure<T>) -> Self {
        Self {
            manifest: AnalyzerManifest {
                id: id.to_owned(),
                requires: Vec::new(),
                scope: Scope::File,
            },
            measure,
        }
    }
}

impl<T: Serialize + Send + Sync> Analyzer for PerFunction<T> {
    fn manifest(&self) -> &AnalyzerManifest {
        &self.manifest
    }

    fn run(&self, ctx: &Ctx) -> Result<Value, Error> {
        let (file, _) = ctx
            .file
            .ok_or_else(|| Error::Failed(format!("{} needs a file", self.manifest.id)))?;
        let values: Vec<Measured<T>> = ctx
            .project
            .symbols_in(&file.path)
            .filter_map(|symbol| {
                let summary = ctx.project.function(&symbol.id)?;
                Some(Measured {
                    symbol: symbol.id.clone(),
                    value: (self.measure)(ctx.project, symbol, summary),
                })
            })
            .collect();
        serde_json::to_value(values).map_err(|e| Error::Failed(e.to_string()))
    }
}

/// Reads the metric fact of `analyzer` for the focused file, by function.
pub fn read<T: DeserializeOwned>(
    ctx: &Ctx,
    analyzer: &str,
) -> Result<BTreeMap<SymbolId, T>, Error> {
    let all: Vec<Measured<T>> = ctx.fact(analyzer)?;
    Ok(all.into_iter().map(|m| (m.symbol, m.value)).collect())
}

/// A body that is one multi-way branch with a single return per arm: a table
/// in code form, long but not complex.
pub fn is_dispatcher(summary: &FunctionSummary) -> bool {
    summary.top_level == 1
        && summary
            .flow
            .first()
            .is_some_and(|f| f.kind == FlowKind::Switch && f.nesting == 0 && f.returning)
}

/// A body whose only top-level branching is one multi-way branch, whatever
/// its arms do, nested at most two levels: many paths, one shape. Its
/// cyclomatic complexity overstates how hard it is to follow; only cognitive
/// complexity can still mark it.
pub fn is_flat_dispatch(summary: &FunctionSummary) -> bool {
    let mut top_level = summary.flow.iter().filter(|f| {
        f.nesting == 0
            && matches!(
                f.kind,
                FlowKind::If
                    | FlowKind::ElseIf
                    | FlowKind::Loop
                    | FlowKind::Catch
                    | FlowKind::Switch
            )
    });
    summary.max_nesting <= FLAT_DISPATCH_NESTING
        && top_level.next().is_some_and(|f| f.kind == FlowKind::Switch)
        && top_level.next().is_none()
}

fn size(_: &Project, symbol: &Symbol, summary: &FunctionSummary) -> Size {
    Size {
        statements: summary.statements,
        lines: symbol.span.end.line - symbol.span.start.line + 1,
    }
}

/// McCabe 1976: one path plus every decision point. Each `if`, `else if`,
/// loop and handler counts one, a switch counts its non-default arms, and
/// every boolean operator counts one.
fn cyclomatic(_: &Project, _: &Symbol, summary: &FunctionSummary) -> u32 {
    1 + summary
        .flow
        .iter()
        .map(|flow| match flow.kind {
            FlowKind::If | FlowKind::ElseIf | FlowKind::Loop | FlowKind::Catch => 1,
            FlowKind::Switch => flow.arms,
            FlowKind::Logic => flow.operators,
            FlowKind::Else | FlowKind::Jump | FlowKind::Recursion => 0,
        })
        .sum::<u32>()
}

fn nesting(_: &Project, _: &Symbol, summary: &FunctionSummary) -> u32 {
    summary.max_nesting
}
