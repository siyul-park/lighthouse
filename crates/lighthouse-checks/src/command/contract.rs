//! What a `command` check and a `command` fix have in common under the process
//! contract: the environment they get and the way placeholder arguments fill
//! in.

use std::{collections::BTreeMap, env, path::Path};

use serde_json::{Map, Value};

/// The version of the contract between Lighthouse and its commands.
const API_VERSION: &str = "1";
/// What a command inherits from its caller, besides what it declares.
const INHERITED: [&str; 4] = ["PATH", "HOME", "LANG", "TMPDIR"];

/// The environment a command gets and nothing else: the inherited allowlist,
/// the declared variables and the context Lighthouse passes.
pub(crate) fn environment(
    declared: &BTreeMap<String, String>,
    decision: &Value,
    options: &Map<String, Value>,
) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = INHERITED
        .iter()
        .filter_map(|k| env::var(k).ok().map(|v| ((*k).to_owned(), v)))
        .collect();
    vars.extend(declared.iter().map(|(k, v)| (k.clone(), v.clone())));
    vars.push(("LIGHTHOUSE_DECISION".to_owned(), decision.to_string()));
    vars.push((
        "LIGHTHOUSE_OPTIONS".to_owned(),
        Value::Object(options.clone()).to_string(),
    ));
    vars.push(("LIGHTHOUSE_API_VERSION".to_owned(), API_VERSION.to_owned()));
    vars
}

/// The command line: an argument `fill` gives values for is replaced by them
/// as a whole, so no value can grow into an option, and a value that starts
/// with `-` is refused. A program named by a relative path is the project's
/// own, found from `root`.
pub(crate) fn expand(
    argv: &[String],
    root: &Path,
    fill: impl Fn(&str) -> Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    let mut expanded = Vec::new();
    for (at, arg) in argv.iter().enumerate() {
        if at == 0 && arg.contains('/') && !Path::new(arg).is_absolute() {
            expanded.push(root.join(arg).to_string_lossy().into_owned());
            continue;
        }
        let values = fill(arg).unwrap_or_else(|| vec![arg.clone()]);
        if let Some(bad) = values.iter().find(|v| *v != arg && v.starts_with('-')) {
            return Err(format!(
                "the value `{bad}` of `{arg}` starts with `-`, so it is refused"
            ));
        }
        expanded.extend(values);
    }
    Ok(expanded)
}
