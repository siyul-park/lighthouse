//! Where user-level files live, by the XDG Base Directory Specification: each
//! kind has a variable and a default below `$HOME`; a relative value of the
//! variable is invalid and ignored.

use std::{env, ffi::OsString, path::PathBuf};

/// The directory below each base that is Lighthouse's.
const APP: &str = "lighthouse";

/// A kind of user-level file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// What the user decided: `trust.toml`.
    Config,
    /// What the user installed: plugins.
    Data,
    /// What can be fetched or built again: downloaded model weights.
    Cache,
}

impl Base {
    fn variable(self) -> &'static str {
        match self {
            Self::Config => "XDG_CONFIG_HOME",
            Self::Data => "XDG_DATA_HOME",
            Self::Cache => "XDG_CACHE_HOME",
        }
    }

    fn fallback(self) -> &'static str {
        match self {
            Self::Config => ".config",
            Self::Data => ".local/share",
            Self::Cache => ".cache",
        }
    }
}

/// `$XDG_<KIND>_HOME/lighthouse`, else `$HOME/<default>/lighthouse`; `None`
/// when neither is usable.
pub fn dir(base: Base) -> Option<PathBuf> {
    dir_in(base, |name| env::var_os(name))
}

/// [`dir`] reading the variables through `var`.
pub fn dir_in(base: Base, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let root = absolute(base.variable())
        .or_else(|| absolute("HOME").map(|home| home.join(base.fallback())))?;
    Some(root.join(APP))
}

/// The plugin directory of the user: `<data>/plugins`.
pub fn plugins() -> Option<PathBuf> {
    dir(Base::Data).map(|data| data.join("plugins"))
}
