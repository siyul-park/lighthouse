//! The `pair` of a test file, the source file it is named after, and the
//! lists a provider states of a file.

use std::path::Path;

use lighthouse_model::ucm::File;
use serde_json::{Value, json};

use super::Builder;
use crate::facts;

/// The stem suffix of a test file, after which comes the source file's stem.
const TEST_SUFFIX: &str = "_test";

impl Builder<'_> {
    /// The facts of a file that a provider states or that name another file:
    /// `blocks`, `inline_bodies` and `pair`, each when a check reads it.
    pub(super) fn file_facts(&self, value: &mut Value, file: &File) {
        for name in ["blocks", "inline_bodies"] {
            if self.needs.field(name) {
                value[name] = self.provider_list(file, name);
            }
        }
        if self.needs.field("pair") {
            value["pair"] = self.pair(file);
        }
    }

    /// A list a provider states in the file fact `name` as JSON (the
    /// `const` and `var` declarations as `blocks`, the functions with a body
    /// on one line as `inline_bodies`); empty when it states none.
    pub(super) fn provider_list(&self, file: &File, name: &str) -> Value {
        file.facts
            .get(name)
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_else(|| json!([]))
    }

    /// `{ base, expected, found, sourced }` for a test file named `<base>_test.<ext>`:
    /// the base name, the path of the source file the name asks for and the
    /// path of the source file it is paired with when the project has one
    /// (else empty), and whether the directory has any source file; null for
    /// any other file. A test file is paired with the source file of its
    /// name, else with a source file of its directory whose stem is the
    /// test file's: a provider states the stem of a file (Go's
    /// `x_arm64.go` and `x_other.go` are both `x`, and so is the test file `x_test.go`).
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
        let stem = facts::file_stem(self.project, &file.path);
        let stem = stem.strip_suffix(TEST_SUFFIX);
        let found = self
            .project
            .file(&expected)
            .filter(|f| !f.test)
            .or_else(|| {
                self.project.files.iter().find(|f| {
                    !f.test
                        && f.path.parent() == file.path.parent()
                        && f.path.extension() == file.path.extension()
                        && Some(facts::file_stem(self.project, &f.path).as_str()) == stem
                })
            })
            .map_or_else(String::new, |f| path(&f.path));
        let sourced = self.project.files.iter().any(|f| {
            !f.test
                && f.path.parent() == file.path.parent()
                && f.path.extension() == file.path.extension()
        });
        json!({ "base": base, "expected": path(&expected), "found": found, "sourced": sourced })
    }
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
