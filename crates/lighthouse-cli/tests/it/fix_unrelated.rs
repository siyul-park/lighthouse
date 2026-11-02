//! A file nobody edited does not undo the fixes of the others: one that does
//! not parse, one of a language without a fix or a generated one, named among
//! the paths to fix, or left in the project, rolls nothing back.

use predicates::prelude::*;

use crate::fix::{lighthouse, read, rust_project, write};

/// A function that reads a constant declared after it: `declaration-groups`
/// moves the constant above it.
fn misordered(n: u32) -> String {
    format!(
        "/// A.\npub fn a{n}() -> i32 {{\n    LIMIT\n}}\n\n/// The limit.\nconst LIMIT: i32 = {n};\n"
    )
}

const BROKEN: &str = "pub fn broken( {\n";

#[test]
fn a_file_that_does_not_parse_rolls_nothing_back() {
    let dir = rust_project(&misordered(1), "");
    write(dir.path(), "src/one.rs", &misordered(2));
    write(dir.path(), "src/two.rs", &misordered(3));
    write(dir.path(), "src/broken.rs", BROKEN);

    lighthouse(dir.path())
        .args([
            "check",
            "--fix",
            "--decision",
            "design/declaration-groups",
            "src/lib.rs",
            "src/one.rs",
            "src/two.rs",
            "src/broken.rs",
        ])
        .assert()
        .stderr(predicate::str::contains("rolled back").not());

    for file in ["src/lib.rs", "src/one.rs", "src/two.rs"] {
        let fixed = read(&dir, file);
        assert!(
            fixed.find("const LIMIT").unwrap() < fixed.find("pub fn a").unwrap(),
            "{file}: {fixed}"
        );
    }
    assert_eq!(read(&dir, "src/broken.rs"), BROKEN);
}
