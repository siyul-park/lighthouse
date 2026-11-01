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

/// Qualifies, in `parts`, the modules whose path is declared by fragments of
/// more than one language, and everything that names them.
pub(super) fn qualify(parts: &mut [Fragment]) {
    // Only the modules of files of their own: what the regions of a host file
    // declare is stated by the host, and is the project's module when it
    // shares the path.
    let language_of = |part: &Fragment, _: &super::Module| {
        part.files
            .first()
            .map(|f| f.lang.clone())
            .unwrap_or_default()
    };
    let mut languages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for part in parts.iter() {
        for module in part.modules.iter().filter(|m| m.language.is_none()) {
            languages
                .entry(module.path.clone())
                .or_default()
                .insert(language_of(part, module));
        }
    }
    for part in parts.iter_mut() {
        let renames: Vec<(String, String)> = part
            .modules
            .iter()
            .filter(|m| m.language.is_none())
            .filter(|m| languages.get(&m.path).is_some_and(|set| set.len() > 1))
            .map(|m| {
                let language = language_of(part, m);
                (m.path.clone(), format!("{}[{language}]", m.path))
            })
            .collect();
        for (from, to) in &renames {
            Swap { from, to }.fragment(part);
        }
    }
}
