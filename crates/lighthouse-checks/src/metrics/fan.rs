use lighthouse_model::{Project, Symbol, SymbolId, SymbolKind};
use serde::{Deserialize, Serialize};

/// Direct fan-in and fan-out (Henry and Kafura 1981) among the functions and
/// methods of one module; calls from test files do not count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fan {
    pub fan_in: u32,
    pub fan_out: u32,
}

pub(crate) fn measure(project: &Project, symbol: &Symbol) -> Fan {
    let module = symbol.id.module();
    let callable = |id: &&SymbolId| {
        id.module() == module
            && project
                .symbol(id)
                .is_some_and(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
    };
    let production = |id: &&SymbolId| !project.in_test(id);
    let count = |ids: &[SymbolId], keep: &dyn Fn(&&SymbolId) -> bool| {
        let n = ids.iter().filter(callable).filter(keep).count();
        u32::try_from(n).unwrap_or(u32::MAX)
    };
    Fan {
        fan_in: count(project.callers(&symbol.id), &production),
        fan_out: count(project.callees(&symbol.id), &|_| true),
    }
}
