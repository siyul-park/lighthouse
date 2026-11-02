use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How generated code is told apart: the same shape in a project, in a
/// plugin manifest and in anything `extends` brings; both lists merge as the
/// union. Whether generated code is a subject is not said here: a decision's
/// `scope.generated` and a rule's `generated` say that.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeneratedSpec {
    /// Patterns of project-relative paths that are generated code: the syntax
    /// and the matcher of `ignores`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Regular expressions searched in the comments at the head of a file (the
    /// ones before its first symbol), delimiters included, whatever its
    /// language: a file one of them matches is generated code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,
}

impl GeneratedSpec {
    /// Whether it says nothing about generated code.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.markers.is_empty()
    }
}
