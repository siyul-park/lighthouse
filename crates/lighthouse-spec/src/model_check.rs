//! The `model` check: a decision only a model can judge. It is the same
//! runtime as a `cel` check whose `where` is false and whose `review` is the
//! `select`: the subjects `select` picks are put to the models the decision is
//! bound to.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Binding, CelCheck, Identity, Select, Subject};

/// P(fail) at which an answer is a `fail` when the check names none.
pub const DEFAULT_THRESHOLD: f64 = 0.9;

/// A check a model answers. The subjects `select` picks are put to the models
/// the decision is bound to, with the decision's examples as their few shots.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelCheck {
    /// What the check ranges over. Default: what the decision's
    /// `scope.subject` is about. Only subjects of a file can be judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<Select>,
    /// Named intermediate values for `select`, evaluated in order.
    #[serde(default, rename = "with", skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
    /// CEL over the ranged value that picks the subjects worth asking about;
    /// aim for recall. Default: every subject that matches the decision's scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<String>,
    /// What the model is told. Default: the decision's requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// P(fail), from 0 to 1, at or above which a model's answer is a `fail`;
    /// below it the subject is left to review. Default: 0.9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// What makes a finding the same one from run to run, as for a `cel` check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

impl ModelCheck {
    /// The `cel` check this one is: never a finding by itself, every selected
    /// subject goes to the models, and a `fail` is reported with the model's
    /// reason. `None` for a decision that names nothing to range over.
    pub fn as_cel(&self, subject: Subject) -> Option<CelCheck> {
        let over = self.over.or_else(|| Select::of(subject))?;
        Some(CelCheck {
            select: Some(over),
            bindings: self.bindings.clone(),
            condition: "false".to_owned(),
            message: String::new(),
            evidence: BTreeMap::new(),
            at: None,
            identity: self.identity.clone(),
            review: Some(self.select.clone().unwrap_or_else(|| "true".to_owned())),
            prompt: self.prompt.clone(),
            threshold: self.threshold,
        })
    }

    /// The first problem with the check for a decision about `subject`.
    pub(crate) fn problem(&self, subject: Subject) -> Option<String> {
        // A decision about a project has nothing to range over by default and
        // stays with agent review; its `select` must still compile.
        let Some(cel) = self.as_cel(subject) else {
            let source = self.select.as_deref()?;
            return cel::Program::compile(source)
                .err()
                .map(|e| format!("check select: {e}"));
        };
        cel.problem()
            .map(|p| p.replacen("check review", "check select", 1))
    }
}
