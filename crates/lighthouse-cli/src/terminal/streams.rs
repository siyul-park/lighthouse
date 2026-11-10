//! Where a command prints and what it reads.

use std::io::Write;

/// What a command prints to, and what it reads from its standard input when
/// it reads any.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub stdin: Option<String>,
}

impl<'a> Io<'a> {
    pub fn new(out: &'a mut dyn Write, err: &'a mut dyn Write, stdin: Option<String>) -> Self {
        Self { out, err, stdin }
    }
}
