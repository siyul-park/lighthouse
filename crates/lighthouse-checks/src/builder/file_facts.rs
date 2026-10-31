//! The `pair` of a test file, the source file it is named after, and the
//! `blocks` of a file, the declarations its provider groups.

use std::path::Path;

use lighthouse_model::ucm::File;
use serde_json::{Value, json};

use super::Builder;

/// The file fact in which a provider lists the `const` and `var` declarations.
const BLOCKS_FACT: &str = "blocks";

/// The stem suffix of a test file, after which comes the source file's stem.
const TEST_SUFFIX: &str = "_test";

impl Builder<'_> {
    /// The `const` and `var` declarations of the file as its provider states
    /// them in the file fact `blocks` (a JSON list of `{ keyword, line, specs
    /// }`); empty when it states none.
    pub(super) fn blocks(&self, file: &File) -> Value {
        file.facts
            .get(BLOCKS_FACT)
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_else(|| json!([]))
    }

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
