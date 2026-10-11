// ---- Helpers ----

/// Runs the thing.
#[inline]
pub fn run() -> &'static str {
    let x = "// not a comment"; // trailing
    x
}

/* block /* nested */ comment */
pub const BLOCK: u8 = 2;
