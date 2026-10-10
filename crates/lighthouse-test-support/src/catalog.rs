//! Catalogs written as files, the way tests build them: a pack `p` with one
//! section `s` and one decision `p/a`, and the documents around them.

use std::collections::BTreeMap;

/// The spec of decision `p/a`, indented as it sits under `spec:`.
pub const SPEC: &str = "  title: A
  intent: i
  scope: { subject: file }
  requirement: A MUST b.
";

/// What a checked decision adds to `SPEC`: an authored severity and evidence.
pub const CHECKED_SPEC: &str = "  severity: error
  evidence: [x]
";

/// Catalog files keyed by `/`-separated path.
pub type Files = BTreeMap<String, String>;

/// A `Decision` document for `id` listed in `section`, with `spec` under
/// `spec:` (already indented by two spaces).
pub fn decision(id: &str, section: &str, spec: &str) -> String {
    let pack = id.split_once('/').map_or(id, |(pack, _)| pack);
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: {id}\n  labels:\n    lighthouse/pack: {pack}\n    lighthouse/section: {section}\nspec:\n{spec}"
    )
}

/// A `Pack` document: each section with the decisions it lists.
pub fn pack(id: &str, sections: &[(&str, &[&str])]) -> String {
    let mut out = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: {id}\nspec:\n  title: {}\n  intro: x\n  sections:{}\n",
        id.to_uppercase(),
        if sections.is_empty() { " []" } else { "" }
    );
    for (name, decisions) in sections {
        out.push_str(&format!(
            "    - name: {name}\n      title: {}\n      intro: x\n      decisions: [{}]\n",
            name.to_uppercase(),
            decisions.join(", ")
        ));
    }
    out
}

/// `entries` as catalog files.
pub fn files(entries: &[(&str, String)]) -> Files {
    entries
        .iter()
        .map(|(path, text)| ((*path).to_owned(), text.clone()))
        .collect()
}

/// Pack `p`, section `s`, decision `p/a`.
pub fn base() -> Files {
    files(&[
        ("p/pack.yaml", pack("p", &[("s", &["a"])])),
        ("p/s/a.yaml", decision("p/a", "s", SPEC)),
    ])
}

/// `base` with `path` set to `text`.
pub fn with(path: &str, text: &str) -> Files {
    let mut files = base();
    files.insert(path.to_owned(), text.to_owned());
    files
}

/// `base` with decision `p/a` having `extra` after its spec lines; a `check:`
/// in `extra` brings an `error` severity and evidence with it.
pub fn decision_with(extra: &str) -> Files {
    let head = if extra.contains("  check:") {
        format!("{SPEC}{CHECKED_SPEC}")
    } else {
        SPEC.to_owned()
    };
    with(
        "p/s/a.yaml",
        &decision("p/a", "s", &format!("{head}{extra}")),
    )
}
