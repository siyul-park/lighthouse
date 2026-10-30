//! The environment variables Lighthouse reads about the user, through one
//! lookup. A process that opens sessions itself (a test) replaces some of them
//! once, instead of changing the environment other threads read.

use std::{collections::BTreeMap, env, ffi::OsString, sync::OnceLock};

static REPLACED: OnceLock<BTreeMap<String, Option<OsString>>> = OnceLock::new();

/// The value of the variable `name`: the replaced one when the process
/// replaced it (`None` is a variable it unset), else the environment's.
pub fn var_os(name: &str) -> Option<OsString> {
    match REPLACED.get().and_then(|replaced| replaced.get(name)) {
        Some(value) => value.clone(),
        None => env::var_os(name),
    }
}

/// [`var_os`] as text; `None` when unset or not Unicode.
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|value| value.into_string().ok())
}

/// Replaces variables for the rest of the process: each name takes its value,
/// `None` unsets it, and the others stay the environment's. Only the first
/// call takes effect; later ones return false.
pub fn replace_vars(vars: impl IntoIterator<Item = (String, Option<OsString>)>) -> bool {
    REPLACED.set(vars.into_iter().collect()).is_ok()
}
