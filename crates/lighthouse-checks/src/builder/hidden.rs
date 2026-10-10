//! The `hidden_target` fact: the symbol a test is named after, when the test
//! reaches it only through a helper of the test code.

use std::collections::BTreeSet;

use lighthouse_model::{EdgeKind, Node, Symbol, SymbolId, SymbolKind, Target};
use serde_json::{Value, json};

use super::Builder;

impl Builder<'_> {
    /// The target the test's name maps to that the test does not call or
    /// reference itself but a helper it calls does: `{target, helper, site}`,
    /// empty when the test uses what it is named after, or is named after
    /// nothing it reaches. Only helpers the test calls directly are followed,
    /// and calls inside closures or `t.Run` helpers count as the test's own. A helper that calls the target is taken to return its result
    /// or assert on it; the code model does not say which.
    pub(super) fn hidden_target(&self, test: &Symbol) -> Value {
        let project = self.project;
        let (Some(naming), Some(case)) = (&self.naming, project.test(&test.id)) else {
            return json!({});
        };
        let mut direct: BTreeSet<&SymbolId> = project
            .uses(&test.id)
            .iter()
            .filter(|id| !project.in_test(id))
            .collect();
        direct.extend(case.targets.iter().filter_map(|target| match target {
            Target::Resolved(Node::Symbol(id)) if !project.in_test(id) => Some(id),
            _ => None,
        }));
        for helper in project
            .callees(&test.id)
            .iter()
            .filter(|id| self.is_helper(id))
        {
            let hidden = project
                .callees(helper)
                .iter()
                .filter(|id| !project.in_test(id) && !direct.contains(id))
                .filter_map(|id| project.symbol(id))
                .find(|target| {
                    naming
                        .credited_tests(target)
                        .iter()
                        .any(|(credited, _)| credited.symbol == test.id)
                });
            if let Some(target) = hidden {
                return json!({
                    "target": target.id.as_str(),
                    "target_name": target.name,
                    "helper": helper.as_str(),
                    "helper_name": project.symbol(helper).map_or("", |h| h.name.as_str()),
                    "site": self.call_line(&test.id, helper),
                });
            }
        }
        json!({})
    }

    /// The line where `from` calls `callee`; 0 when the provider reports no sites.
    fn call_line(&self, from: &SymbolId, callee: &SymbolId) -> u32 {
        self.project
            .sites(callee)
            .iter()
            .filter(|site| site.kind == EdgeKind::Calls)
            .filter(|site| matches!(&site.from, Node::Symbol(id) if id == from))
            .map(|site| site.span.start.line)
            .min()
            .unwrap_or(0)
    }

    /// Whether `id` is a function of test code that is not a test case itself.
    fn is_helper(&self, id: &SymbolId) -> bool {
        let project = self.project;
        project.in_test(id)
            && project.test(id).is_none()
            && project
                .symbol(id)
                .is_some_and(|s| s.kind == SymbolKind::Function)
    }
}
