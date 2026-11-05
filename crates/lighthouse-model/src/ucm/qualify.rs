//! Module ids that are unique across languages. The id of a module is its
//! path, and files of different languages can have the same one (`x.js` and
//! `x.py` are both `x`); their symbols would then share ids and only the first
//! be analyzed. Each of the colliding modules gets its language: `x[python]`.
//! Ids without a collision stay as they are.

use std::collections::{BTreeMap, BTreeSet};

use super::{Fragment, Node, SymbolId, Target};

/// The replacement of the module `from` by `to` in the ids of a fragment.
struct Swap<'a> {
    from: &'a str,
    to: &'a str,
}

impl Swap<'_> {
    fn fragment(&self, part: &mut Fragment) {
        for module in &mut part.modules {
            self.text(&mut module.path);
            if let Some(of) = &mut module.test_of {
                self.text(of);
            }
        }
        for symbol in &mut part.symbols {
            self.id(&mut symbol.id);
            if let Some(owner) = &mut symbol.owner {
                self.id(owner);
            }
        }
        for edge in &mut part.edges {
            self.node(&mut edge.from);
            self.target(&mut edge.to);
        }
        for function in &mut part.functions {
            self.id(&mut function.symbol);
            if let Some(forwards) = &mut function.forwards_to {
                self.target(forwards);
            }
        }
        for test in &mut part.tests {
            self.id(&mut test.symbol);
            test.targets.iter_mut().for_each(|t| self.target(t));
        }
        for symbol in part
            .comments
            .iter_mut()
            .filter_map(|c| c.attached_to.as_mut())
        {
            self.id(symbol);
        }
    }

    /// `text` when it names the module or something in it: the same with the
    /// module replaced.
    fn text(&self, text: &mut String) {
        if *text == self.from {
            self.to.clone_into(text);
        } else if let Some(rest) = text
            .strip_prefix(self.from)
            .and_then(|rest| rest.strip_prefix("::"))
        {
            *text = format!("{}::{rest}", self.to);
        }
    }

    fn id(&self, id: &mut SymbolId) {
        self.text(&mut id.0);
    }

    fn node(&self, node: &mut Node) {
        match node {
            Node::Module(path) => self.text(path),
            Node::Symbol(symbol) => self.id(symbol),
        }
    }

    fn target(&self, target: &mut Target) {
        match target {
            Target::Resolved(node) => self.node(node),
            Target::Path(path) => self.text(path),
        }
    }
}

/// The modules `part` declares as a file of its own, each with the language of
/// that file: what decides whether a path is declared in several languages.
pub fn declared(part: &Fragment) -> impl Iterator<Item = (String, String)> + '_ {
    let language = part
        .files
        .first()
        .map(|f| f.lang.clone())
        .unwrap_or_default();
    // Only the modules of files of their own: what the regions of a host file
    // declare is stated by the host, and is the project's module when it
    // shares the path.
    part.modules
        .iter()
        .filter(|m| m.language.is_none())
        .map(move |m| (m.path.clone(), language.clone()))
}

/// The module paths that are declared in more than one language, from what
/// every fragment [`declared`].
pub fn collisions(declared: impl IntoIterator<Item = (String, String)>) -> BTreeSet<String> {
    let mut languages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, language) in declared {
        languages.entry(path).or_default().insert(language);
    }
    languages
        .into_iter()
        .filter(|(_, set)| set.len() > 1)
        .map(|(path, _)| path)
        .collect()
}

/// Qualifies, in `part`, the modules of `collisions` that it declares, and
/// everything that names them.
pub fn qualify_part(part: &mut Fragment, collisions: &BTreeSet<String>) {
    let renames: Vec<(String, String)> = declared(part)
        .filter(|(path, _)| collisions.contains(path))
        .map(|(path, language)| {
            let to = format!("{path}[{language}]");
            (path, to)
        })
        .collect();
    for (from, to) in &renames {
        Swap { from, to }.fragment(part);
    }
}

/// Qualifies, in `parts`, the modules whose path is declared by fragments of
/// more than one language, and everything that names them.
pub(super) fn qualify(parts: &mut [Fragment]) {
    let collisions = collisions(parts.iter().flat_map(declared));
    for part in parts {
        qualify_part(part, &collisions);
    }
}
