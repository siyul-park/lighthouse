//! The generated documentation of the bundled catalog.

use std::collections::BTreeMap;

use lighthouse_spec::{Catalog, DOCS_DIR, docs, fix_operations_markdown};

/// Every generated page by path under the docs directory: one per pack, and
/// the fix operations with the order keys the bundled plugins register.
pub fn bundled_docs() -> BTreeMap<String, String> {
    let mut pages = docs(Catalog::bundled());
    let keys: Vec<(String, String)> = lighthouse_checks::registry()
        .order_keys()
        .map(|k| {
            let manifest = k.manifest();
            (manifest.id.clone(), manifest.description.clone())
        })
        .collect();
    pages.insert(
        format!("{DOCS_DIR}/fix-operations.md"),
        fix_operations_markdown(&keys),
    );
    pages
}
