use lighthouse_model::{FlowKind, FunctionSummary, Project, Symbol};

/// Cognitive Complexity (Campbell, SonarSource 2018): constructs that break
/// linear flow add 1 and, for structures that nest, the nesting level at
/// which they occur; `else if`, `else`, jumps, boolean operator runs and
/// recursion add 1 flat.
pub(crate) fn score(_: &Project, _: &Symbol, summary: &FunctionSummary) -> u32 {
    summary
        .flow
        .iter()
        .map(|flow| match flow.kind {
            FlowKind::If | FlowKind::Switch | FlowKind::Loop | FlowKind::Catch => 1 + flow.nesting,
            FlowKind::ElseIf
            | FlowKind::Else
            | FlowKind::Jump
            | FlowKind::Logic
            | FlowKind::Recursion => 1,
        })
        .sum()
}
