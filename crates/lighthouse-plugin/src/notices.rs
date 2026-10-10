use std::sync::{Mutex, PoisonError};

/// What a rule tells the user besides its findings: a result it dropped, a
/// file it could not read. The engine drains it after the rule ran and puts
/// the lines among the run's notices, so that nothing is dropped silently.
#[derive(Default)]
pub struct Notices {
    lines: Mutex<Vec<String>>,
}

impl Notices {
    /// Adds a notice.
    pub fn push(&self, line: impl Into<String>) {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.into());
    }

    /// Takes the notices pushed so far.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.lines.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
