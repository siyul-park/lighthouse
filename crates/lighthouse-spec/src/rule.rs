use lighthouse_plugin::{RuleMeta, Scope as RunScope};

use crate::{Pattern, Scope};

impl Scope {
    /// Symbol, file and test patterns are checked once per file; module and
    /// project patterns once over the merged project.
    pub fn rule_scope(self) -> RunScope {
        match self {
            Self::Symbol | Self::File | Self::Test => RunScope::File,
            Self::Module | Self::Project => RunScope::Project,
        }
    }
}

impl Pattern {
    /// Rule metadata of an implemented pattern; `None` while it has no
    /// implementation. Analyzers and capabilities belong to the implementation.
    pub fn rule_meta(&self) -> Option<RuleMeta> {
        self.implementation.as_ref()?;
        Some(RuleMeta {
            id: self.id.clone(),
            severity: self.severity()?,
            scope: self.scope.rule_scope(),
            description: self.title.clone(),
            docs: self.requirement.clone(),
            analyzers: Vec::new(),
            capabilities: Vec::new(),
            citation: self.citation.clone(),
        })
    }
}
