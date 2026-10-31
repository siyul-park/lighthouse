//! The `pair` of a test file: the source file it is named after.

use std::path::Path;

use lighthouse_model::ucm::File;
use serde_json::{Value, json};

use super::Builder;

/// The stem suffix of a test file, after which comes the source file's stem.
const TEST_SUFFIX: &str = "_test";

impl Builder<'_> {
    /// `{ base, expected, found }` for a test file named `<base>_test.<ext>`:
    /// the base name, the path of the source file the name asks for and the
    /// path of that file when the project has it (else empty); null for any
    /// other file.
    pub(super) fn pair(&self, file: &File) -> Value {
        let Some(base) = file
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|_| file.test)
            .and_then(|s| s.strip_suffix(TEST_SUFFIX))
        else {
            return Value::Null;
        };
        let extension = file
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let expected = file.path.with_file_name(format!("{base}.{extension}"));
        let found = self
            .project
            .file(&expected)
            .filter(|f| !f.test)
            .map_or_else(String::new, |f| path(&f.path));
        json!({ "base": base, "expected": path(&expected), "found": found })
    }
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
