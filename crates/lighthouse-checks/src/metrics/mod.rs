pub(crate) mod graph;
pub mod propagation;

use std::{collections::BTreeMap, sync::LazyLock};

use lighthouse_model::{
    RunScope,
    ucm::{FlowKind, FunctionSummary, Project, Symbol, SymbolId, SymbolKind},
};
use lighthouse_plugin::{Analyzer, AnalyzerManifest, Context, Error, Plugin, PluginManifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Plugin id.
const ID: &str = "metrics";
/// Analyzer id of the per-function [`Measures`] fact: a map from the functions
/// and methods declared in the focused file to what is measured of each.
pub const MEASURES: &str = "metrics/function";

/// Deepest nesting of a flat dispatch's arms.
const FLAT_DISPATCH_NESTING: u32 = 2;

/// What is measured of one function.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measures {
    /// Lines of the declaration, inclusive of both ends.
    pub lines: u32,
    pub statements: u32,
    /// McCabe 1976: one path plus every decision point. Each `if`, `else if`,
    /// loop and handler counts one, a switch counts its non-default arms, and
    /// every boolean operator counts one.
    pub cyclomatic: u32,
    /// Cognitive Complexity (Campbell, SonarSource 2018): constructs that
    /// break linear flow add 1 and, for structures that nest, the nesting
    /// level at which they occur; `else if`, `else`, jumps, boolean operator
    /// runs and recursion add 1 flat.
    pub cognitive: u32,
    /// The deepest nesting level.
    pub nesting: u32,
    /// Direct fan-in and fan-out (Henry and Kafura 1981) among the functions
    /// and methods of its module; calls from test files do not count.
    pub fan_in: u32,
    pub fan_out: u32,
    /// Whether the body is [`is_dispatcher`].
    pub dispatcher: bool,
    /// Whether the body is [`is_flat_dispatch`].
    pub flat_dispatch: bool,
}

impl Measures {
    /// Measures `symbol`, whose summary is `summary`.
    pub fn of(project: &Project, symbol: &Symbol, summary: &FunctionSummary) -> Self {
        let (mut cyclomatic, mut cognitive) = (1, 0);
        for flow in &summary.flow {
            cyclomatic += match flow.kind {
                FlowKind::If | FlowKind::ElseIf | FlowKind::Loop | FlowKind::Catch => 1,
                FlowKind::Switch => flow.arms,
                FlowKind::Logic => flow.operators,
                FlowKind::Else | FlowKind::Jump | FlowKind::Recursion => 0,
            };
            cognitive += match flow.kind {
                FlowKind::If | FlowKind::Switch | FlowKind::Loop | FlowKind::Catch => {
                    1 + flow.nesting
                }
                FlowKind::ElseIf
                | FlowKind::Else
                | FlowKind::Jump
                | FlowKind::Logic
                | FlowKind::Recursion => 1,
            };
        }
        let module = symbol.id.module();
        let callable = |id: &&SymbolId| {
            id.module() == module
                && project
                    .symbol(id)
                    .is_some_and(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        };
        let count = |ids: &[SymbolId], keep: &dyn Fn(&&SymbolId) -> bool| {
            u32::try_from(ids.iter().filter(callable).filter(keep).count()).unwrap_or(u32::MAX)
        };
        Self {
            lines: symbol.span.end.line - symbol.span.start.line + 1,
            statements: summary.statements,
            cyclomatic,
            cognitive,
            nesting: summary.max_nesting,
            fan_in: count(project.callers(&symbol.id), &|id| !project.in_test(id)),
            fan_out: count(project.callees(&symbol.id), &|_| true),
            dispatcher: is_dispatcher(summary),
            flat_dispatch: is_flat_dispatch(summary),
        }
    }
}

/// The plugin providing the `metrics/function` analyzer; stateless.
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

    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        vec![Box::new(Self)]
    }
}

/// Measures every function and method declared in the focused file.
impl Analyzer for Metrics {
    fn manifest(&self) -> &AnalyzerManifest {
        static MANIFEST: LazyLock<AnalyzerManifest> = LazyLock::new(|| AnalyzerManifest {
            id: MEASURES.to_owned(),
            requires: Vec::new(),
            scope: RunScope::File,
        });
        &MANIFEST
    }

    fn run(&self, ctx: &Context) -> Result<Value, Error> {
        let (file, _) = ctx
            .file
            .ok_or_else(|| Error::Failed(format!("{MEASURES} needs a file")))?;
        let measures: BTreeMap<&SymbolId, Measures> = ctx
            .project
            .symbols_in(&file.path)
            .filter(|symbol| {
                matches!(
                    symbol.kind,
                    SymbolKind::Function | SymbolKind::Method | SymbolKind::Var
                )
            })
            .filter_map(|symbol| {
                let summary = ctx.project.function(&symbol.id)?;
                Some((&symbol.id, Measures::of(ctx.project, symbol, summary)))
            })
            .collect();
        serde_json::to_value(measures).map_err(|e| Error::Failed(e.to_string()))
    }
}

/// A body that is one multi-way branch with a single return per arm: a table
/// in code form, long but not complex.
fn is_dispatcher(summary: &FunctionSummary) -> bool {
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
fn is_flat_dispatch(summary: &FunctionSummary) -> bool {
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
