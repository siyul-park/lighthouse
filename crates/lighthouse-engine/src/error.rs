use std::{io, path::PathBuf};

use lighthouse_spec::Error as SpecError;
use thiserror::Error;

/// Why a run could not start or finish: invalid configuration, unknown or
/// unlisted plugin or rule, or an unreadable path. Analysis gaps are not
/// errors; they are [`crate::Run::incomplete`].
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Spec(#[from] SpecError),
    #[error(transparent)]
    Plugin(#[from] lighthouse_plugin::Error),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("unknown plugin `{0}`")]
    UnknownPlugin(String),
    #[error("unknown language `{0}` in `languages` configuration")]
    UnknownLanguage(String),
    #[error("unknown source provider `{0}` in sources[].language.provider")]
    UnknownSourceProvider(String),
    #[error("`{0}` belongs to a plugin not listed in `plugins`")]
    PluginNotListed(String),
    #[error("unknown model `{0}`: declare it under `[models.<name>]`")]
    UnknownModel(String),
    #[error("model `{name}`: {reason}")]
    Model { name: String, reason: String },
    #[error("unknown rule `{0}`")]
    UnknownRule(String),
    #[error("rule `{0}` is not enabled by the configuration")]
    RuleNotEnabled(String),
    /// A fix run that cannot start or finish safely: incomplete analysis, a
    /// file changed under it, a formatter that cannot run, an unknown fixer.
    #[error("{0}")]
    Fix(String),
}
