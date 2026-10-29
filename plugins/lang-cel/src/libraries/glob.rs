//! Module path globs as a pattern language exposed like the `regex`
//! extension: `glob.matches(path, pattern)`, matched by
//! `lighthouse_model::glob`.
//!
//! ```text
//! glob.matches('app/http', 'app/*')   == true
//! glob.matches('app/http/x', 'app/*') == false
//! glob.matches('app', 'app/**')       == true
//! ```

use cel::{
    DeclarationError, Env,
    common::types::{CelBool, CelString},
};
use lighthouse_plugin::{Overload, Precedence};

use crate::{Library, function};

/// The library: module path globs.
pub(crate) fn library() -> Library {
    Library::new(
        "glob",
        vec![function(
            "glob.matches",
            None,
            vec![Overload::new(&["string", "string"], "bool")],
            "Whether a module path matches a glob, by segment: `*` is a run of characters inside one segment, `**` any number of segments. `::` reads as `/`.",
            "glob.matches('app/http', 'app/*')",
            Precedence::Designed,
        )],
        install,
    )
}

/// Registers the overload on `env`.
fn install(env: &mut Env) -> Result<(), DeclarationError> {
    cel::add_overload!(env, fn matches_glob: (CelString, CelString) -> CelBool,
        name = "glob.matches")
}

fn matches_glob(path: &CelString<'_>, pattern: &CelString<'_>) -> CelBool {
    CelBool::from(lighthouse_model::glob::matches(path, pattern))
}
