use std::collections::BTreeMap;

use lighthouse_spec::{Config, Projects, Renamed};

/// Renames the rules the configuration and the projects it can extend set
/// under the id a decision had, to the id it has now; returns what the user
/// should know about it, sorted.
pub(crate) fn apply(
    config: &mut Config,
    projects: &mut Projects,
    aliases: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut renamed = config.rename_rules(aliases);
    renamed.extend(projects.rename_rules(aliases));
    let mut notices: Vec<String> = renamed.iter().map(notice).collect();
    notices.sort();
    notices.dedup();
    notices
}

/// What the user should know about a rule the configuration sets by the id a
/// decision had.
fn notice(renamed: &Renamed) -> String {
    let Renamed { old, new, both } = renamed;
    if *both {
        format!(
            "the configuration sets both `{old}` and `{new}` (the decision was renamed); the setting of `{new}` is kept"
        )
    } else {
        format!(
            "decision `{old}` is now `{new}`; the old id still works (`lighthouse spec migrate` rewrites it)"
        )
    }
}
