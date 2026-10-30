/// Why a check could not be built.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{rule}: {message}")]
    Invalid { rule: String, message: String },
    #[error(transparent)]
    Spec(#[from] lighthouse_spec::Error),
}

impl Error {
    pub(crate) fn invalid(rule: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            rule: rule.to_owned(),
            message: message.into(),
        }
    }
}
