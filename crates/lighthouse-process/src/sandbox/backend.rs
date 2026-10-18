//! The backend of this OS.

use super::{Plan, bubblewrap, seatbelt};

/// What to do about a missing backend, for the message that says so.
pub(super) const HINT: &str = if cfg!(target_os = "linux") {
    "install bubblewrap"
} else {
    "run Lighthouse outside another sandbox"
};

/// The exit code of a program that ended with `code` under the backend.
/// Bubblewrap reports a program killed by signal `n` as the code `128 + n`;
/// that is a signal, as it is without the backend.
pub(crate) fn ended(code: Option<i32>) -> Option<i32> {
    const SIGNALS: std::ops::RangeInclusive<i32> = 129..=192;
    code.filter(|code| !(cfg!(target_os = "linux") && SIGNALS.contains(code)))
}

/// `argv` under the sandbox `plan` describes; empty where this OS has none.
pub(crate) fn wrap(plan: &Plan, argv: &[String]) -> Vec<String> {
    if cfg!(target_os = "macos") {
        let mut wrapped = vec![
            "/usr/bin/sandbox-exec".to_owned(),
            "-p".to_owned(),
            seatbelt::profile(plan),
            "--".to_owned(),
        ];
        wrapped.extend(argv.iter().cloned());
        wrapped
    } else if cfg!(target_os = "linux") {
        bubblewrap::argv(plan, argv)
    } else {
        Vec::new()
    }
}
