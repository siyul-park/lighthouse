//! Renames: a symbol and every site that refers to it, when the names the
//! edit would make cannot capture or shadow another.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::{
    LineIndex,
    ucm::{EdgeKind, Node, Resolution, Site, Symbol, SymbolId, SymbolKind, Target, Visibility},
};

use super::{Lowerer, TextEdit, has_word};

/// Words no identifier may be, in the languages with providers.
const KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "crate",
    "default",
    "defer",
    "dyn",
    "else",
    "enum",
    "extern",
    "fallthrough",
    "false",
    "fn",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "impl",
    "import",
    "in",
    "interface",
    "let",
    "loop",
    "map",
    "match",
    "mod",
    "move",
    "mut",
    "package",
    "pub",
    "range",
    "ref",
    "return",
    "select",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "switch",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "var",
    "where",
    "while",
];

impl Lowerer<'_> {
    pub(super) fn rename(&self, id: &SymbolId, name: &str) -> Result<Vec<TextEdit>, String> {
        let symbol = self
            .project
            .symbol(id)
            .ok_or_else(|| format!("`{}` is not in the project", id.as_str()))?;
        self.rename_allowed(symbol, name)?;
        let blocked = self.project.edges.iter().find(|e| {
            e.kind != EdgeKind::Contains
                && e.to == Target::Resolved(Node::Symbol(id.clone()))
                && (e.resolution != Resolution::Semantic || e.site.is_none())
        });
        if let Some(edge) = blocked {
            return Err(format!(
                "a {:?} edge to `{}` is {}, so its references cannot all be found",
                edge.kind,
                symbol.name,
                if edge.resolution == Resolution::Semantic {
                    "without a site"
                } else {
                    "not semantic"
                }
            ));
        }
        // Where the lines start in each file with a site, found once.
        let sites: Vec<_> = self
            .project
            .sites(id)
            .iter()
            .filter(|site| site.kind != EdgeKind::Contains)
            .collect();
        self.captured(symbol, &sites, name)?;
        let mut edits = Vec::new();
        let text = self.sources.text(&symbol.file)?;
        let index = LineIndex::new(&text);
        let declaration = index
            .range(symbol.span)
            .ok_or_else(|| "the declaration is outside its file".to_owned())?;
        let at = name_in(&text[declaration.clone()], &symbol.name)
            .ok_or_else(|| format!("cannot find `{}` in its declaration", symbol.name))?;
        let start = declaration.start + at;
        edits.push(TextEdit {
            file: symbol.file.clone(),
            range: start..start + symbol.name.len(),
            text: name.to_owned(),
        });
        let texts = sites
            .iter()
            .map(|site| Ok((site.file.as_path(), self.sources.text(&site.file)?)))
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let indexes: BTreeMap<&Path, LineIndex> = texts
            .iter()
            .map(|(&file, text)| (file, LineIndex::new(text)))
            .collect();
        for site in sites {
            let text = &texts[site.file.as_path()];
            let range = indexes[site.file.as_path()]
                .range(site.span)
                .filter(|r| text[r.clone()] == symbol.name)
                .ok_or_else(|| {
                    format!(
                        "a reference site in {} does not spell `{}`",
                        site.file.display(),
                        symbol.name
                    )
                })?;
            edits.push(TextEdit {
                file: site.file.clone(),
                range,
                text: name.to_owned(),
            });
        }
        edits.sort_by(|a, b| (&a.file, a.range.start).cmp(&(&b.file, b.range.start)));
        edits.dedup();
        Ok(edits)
    }

    /// Why the new name would capture or shadow another where `symbol` is
    /// referred to, if it would: `name` already occurs, as a word, in the
    /// function around a site, or is declared at the top level of a module
    /// that holds a site. Conservative: the words are read as text, so one in
    /// a comment or a string counts.
    fn captured(&self, symbol: &Symbol, sites: &[&Site], name: &str) -> Result<(), String> {
        let mut modules = BTreeSet::new();
        for site in sites {
            let text = self.sources.text(&site.file)?;
            let around = self
                .project
                .symbols_in(&site.file)
                .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
                .filter(|s| {
                    let extent = s.extent.unwrap_or(s.span);
                    extent.start <= site.span.start && site.span.end <= extent.end
                })
                .max_by_key(|s| s.extent.unwrap_or(s.span).start);
            if let Some(function) = around {
                let range = LineIndex::new(&text).range(function.extent.unwrap_or(function.span));
                if range.is_some_and(|range| has_word(&text[range], name)) {
                    return Err(format!(
                        "`{name}` already occurs in `{}`, which refers to `{}`, so the new name could capture or shadow it",
                        function.name, symbol.name
                    ));
                }
            }
            modules.extend(
                self.project
                    .symbols_in(&site.file)
                    .map(|s| s.id.module().to_owned()),
            );
        }
        let taken = self.project.symbols.iter().find(|other| {
            other.id != symbol.id
                && other.name == name
                && other.owner.is_none()
                && modules.contains(other.id.module())
        });
        match taken {
            Some(other) => Err(format!(
                "`{name}` is declared in module `{}`, where `{}` is referred to",
                other.id.module(),
                symbol.name
            )),
            None => Ok(()),
        }
    }

    /// Why `symbol` may not be renamed to `name`, if it may not: the provider
    /// must know every reference, the symbol must be private to its unit, and
    /// the name must be a free identifier that is no keyword.
    fn rename_allowed(&self, symbol: &Symbol, name: &str) -> Result<(), String> {
        let language = self
            .project
            .file(&symbol.file)
            .map_or("", |f| f.lang.as_str());
        if !(self.complete)(language) {
            return Err(format!(
                "the provider of `{language}` does not declare `complete-references`, so every reference to `{}` cannot be known",
                symbol.name
            ));
        }
        if symbol.visibility != Visibility::Private {
            return Err(format!(
                "`{}` is visible outside its unit, so code this project cannot see may use it",
                symbol.name
            ));
        }
        let identifier = name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !identifier || KEYWORDS.contains(&name) {
            return Err(format!("`{name}` is not a usable identifier"));
        }
        if name == symbol.name {
            return Err("the new name is the old one".to_owned());
        }
        let taken = self.project.symbols.iter().any(|other| {
            other.id != symbol.id
                && other.name == name
                && other.id.module() == symbol.id.module()
                && other.owner == symbol.owner
        });
        if taken {
            return Err(format!(
                "`{name}` is already declared next to `{}`",
                symbol.name
            ));
        }
        Ok(())
    }
}

/// Where a symbol's name is first written as a whole word in its declaration,
/// not counting a Go receiver, which comes before the name.
pub(super) fn name_in(declaration: &str, name: &str) -> Option<usize> {
    let skip = declaration
        .strip_prefix("func")
        .map(str::trim_start)
        .filter(|rest| rest.starts_with('('))
        .and_then(|rest| {
            rest.find(')')
                .map(|n| declaration.len() - rest.len() + n + 1)
        })
        .unwrap_or(0);
    let bytes = declaration.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    declaration[skip..].match_indices(name).find_map(|(at, _)| {
        let at = skip + at;
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + name.len()).copied();
        (!before.is_some_and(word) && !after.is_some_and(word)).then_some(at)
    })
}
