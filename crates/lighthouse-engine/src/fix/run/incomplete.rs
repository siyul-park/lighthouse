//! What an incomplete analysis refuses. A fix is refused when the analysis it
//! rests on has a gap: until the findings say what they read, a gap anywhere
//! may be what a fix rests on.

use lighthouse_model::Incomplete;

use crate::Run;

/// Why the analysis of `outcome` is too incomplete for a fix, if it is.
pub(super) fn refusal(outcome: &Run) -> Option<String> {
    outcome.incomplete.first().map(describe)
}

fn describe(gap: &Incomplete) -> String {
    format!(
        "the analysis is incomplete ({}{}{}), so the fix was not made",
        gap.provider
            .as_ref()
            .map_or(String::new(), |p| format!("{p}: ")),
        gap.path
            .as_ref()
            .map_or(String::new(), |p| format!("{}: ", p.display())),
        gap.reason
    )
}
