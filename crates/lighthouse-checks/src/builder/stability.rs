//! The `stability` of a module and the `reach` of a symbol: how many others
//! use it directly and through what they use, in the graphs of the project.

use lighthouse_model::ucm::{Module, Symbol};
use serde_json::{Value, json};

use super::Builder;
use crate::metrics::propagation::{self, Stability, System};

impl Builder<'_> {
    /// `{ ca, ce, instability, vfi, vfo, vfi_share, vfo_share, cyclic,
    /// component, core }` on the graph of modules, or null for a module that
    /// is no node: test code or generated.
    pub(super) fn module_stability(&self, module: &Module) -> Value {
        let system = self
            .indexes
            .modules
            .get_or_init(|| propagation::modules(self.project));
        let Some(s) = system.of(&module.path) else {
            return Value::Null;
        };
        let total = s.ca + s.ce;
        let mut value = reached(system, s);
        value["instability"] = if total == 0 {
            Value::Null
        } else {
            json!(round(f64::from(s.ce) / f64::from(total)))
        };
        value["component"] = json!(s.component);
        value["core"] = json!(s.core);
        value
    }

    /// `{ measured, ca, ce, vfi, vfo, vfi_share, vfo_share, cyclic }` on the
    /// graph of symbols; `measured` is false, and the rest zero, for a symbol
    /// that is no node.
    pub(super) fn reach(&self, symbol: &Symbol) -> Value {
        let system = self
            .indexes
            .symbols
            .get_or_init(|| propagation::symbols(self.project));
        let Some(s) = system.of(symbol.id.as_str()) else {
            return json!({
                "measured": false, "ca": 0, "ce": 0, "vfi": 0, "vfo": 0,
                "vfi_share": 0.0, "vfo_share": 0.0, "cyclic": 0,
            });
        };
        let mut value = reached(system, s);
        value["measured"] = json!(true);
        value
    }
}

/// The fields both shapes share.
fn reached(system: &System, s: &Stability) -> Value {
    let n = system.nodes.len() as f64;
    json!({
        "ca": s.ca, "ce": s.ce, "vfi": s.vfi, "vfo": s.vfo,
        "vfi_share": round(f64::from(s.vfi) / n),
        "vfo_share": round(f64::from(s.vfo) / n),
        "cyclic": s.cyclic,
    })
}

/// Four decimals, so that a fact has one text.
fn round(share: f64) -> f64 {
    (share * 1e4).round() / 1e4
}
