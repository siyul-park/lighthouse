use schemars::JsonSchema;
use serde::Serialize;

/// Requests put to models, and what they cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, JsonSchema)]
pub struct Usage {
    /// Requests that went to a model; answers kept from earlier runs are not
    /// counted.
    pub calls: usize,
    pub cost: f64,
}
