//! Lowering: edit operations over code-model nodes become text edits, using
//! the extents and sites language providers report. A Move is a deletion of
//! one block of whole lines and an insertion of the same lines elsewhere, so
//! comments and formatting travel with the declaration as written.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use lighthouse_model::{
    Anchor, EdgeKind, EditOp, LineIndex, Node, Owner, Project, Resolution, Symbol, SymbolId,
    Target, Visibility,
};

use crate::Overlays;

use super::{
    edits::{TextEdit, eol_of, whole_lines, with_blank},
    scan::Containers,
};

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

impl<'a> Sources<'a> {
    pub(crate) fn new(root: &Path, project: &'a Project, overlays: &'a Overlays) -> Self {
        Self {
            root: root.canonicalize().unwrap_or_else(|_| root.to_owned()),
            project,
            overlays,
            texts: RefCell::default(),
            containers: RefCell::default(),
        }
    }

    /// Why `file` may not be edited, if it may not.
    pub(crate) fn contain(&self, file: &Path) -> Result<(), String> {
        let shown = file.display();
        if file.is_absolute()
            || file
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!(
                "`{shown}` is not a relative, normalized project path"
            ));
        }
        match self.project.file(file) {
            None => return Err(format!("`{shown}` is not a file of the project")),
            Some(f) if f.generated => return Err(format!("`{shown}` is generated")),
            Some(_) => {}
        }
        let joined = self.root.join(file);
        match joined.canonicalize() {
            Ok(real) if real == joined => Ok(()),
            Ok(_) => Err(format!("`{shown}` is reached through a symlink")),
            Err(e) => Err(format!("`{shown}`: {e}")),
        }
    }

    pub(crate) fn text(&self, file: &Path) -> Result<Rc<String>, String> {
        if let Some(text) = self.texts.borrow().get(file) {
            return Ok(Rc::clone(text));
        }
        self.contain(file)?;
        let text = match self.overlays.get(file) {
            Some(text) => text.clone(),
            None => fs::read_to_string(self.root.join(file))
                .map_err(|e| format!("{}: {e}", file.display()))?,
        };
        let text = Rc::new(text);
        self.texts
            .borrow_mut()
            .insert(file.to_owned(), Rc::clone(&text));
        Ok(text)
    }

    pub(crate) fn containers(&self, file: &Path) -> Result<Rc<Containers>, String> {
        if let Some(found) = self.containers.borrow().get(file) {
            return Ok(Rc::clone(found));
        }
        let language = self.project.file(file).map_or("", |f| f.lang.as_str());
        let built = Rc::new(Containers::new(&self.text(file)?, language));
        self.containers
            .borrow_mut()
            .insert(file.to_owned(), Rc::clone(&built));
        Ok(built)
    }
}

/// The texts of the files an operation touches: the overlay of the round, else
/// the disk. Every file is checked first: relative and normalized, a
/// non-generated file of the project, and reached through no symlink.
pub(crate) struct Sources<'a> {
    root: PathBuf,
    project: &'a Project,
    overlays: &'a Overlays,
    texts: RefCell<BTreeMap<PathBuf, Rc<String>>>,
    containers: RefCell<BTreeMap<PathBuf, Rc<Containers>>>,
}

/// A declaration located in its file: the symbol, the whole lines its extent
/// fills and the bracket it sits in.
struct Block {
    symbol: Symbol,
    lines: Range<usize>,
    container: Option<usize>,
    group: bool,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(
        project: &'a Project,
        sources: &'a Sources<'a>,
        complete: &'a dyn Fn(&str) -> bool,
    ) -> Self {
        Self {
            project,
            sources,
            complete,
        }
    }

    /// The text edits of one operation, or why it cannot be lowered.
    pub(crate) fn lower(&self, op: &EditOp) -> Result<Vec<TextEdit>, String> {
        match op {
            EditOp::Move { node, anchor } => self.relocate(node, anchor),
            EditOp::Delete { node } => {
                let block = self.block(node)?;
                let text = self.sources.text(&block.symbol.file)?;
                let range = with_blank(&text, block.lines.clone());
                Ok(vec![TextEdit::delete(block.symbol.file, range)])
            }
            EditOp::Reorder { owner, order } => self.reorder(owner, order),
            EditOp::Rename { symbol, name } => self.rename(symbol, name),
            EditOp::DeleteRange { file, span } => {
                let text = self.sources.text(file)?;
                let range = LineIndex::new(&text)
                    .range(*span)
                    .ok_or_else(|| format!("{}: span is outside the file", file.display()))?;
                let range =
                    whole_lines(&text, &range).unwrap_or_else(|| trimmed_back(&text, range));
                Ok(vec![TextEdit::delete(file.clone(), range)])
            }
            EditOp::Replace { file, span, text } => {
                let source = self.sources.text(file)?;
                let range = LineIndex::new(&source)
                    .range(*span)
                    .ok_or_else(|| format!("{}: span is outside the file", file.display()))?;
                Ok(vec![TextEdit {
                    file: file.clone(),
                    range,
                    text: text.clone(),
                }])
            }
        }
    }

    fn block(&self, node: &Node) -> Result<Block, String> {
        let Node::Symbol(id) = node else {
            return Err("only symbols can be moved or deleted, not modules".to_owned());
        };
        let symbol = self
            .project
            .symbol(id)
            .ok_or_else(|| format!("`{}` is not in the project", id.as_str()))?;
        let extent = symbol.extent.ok_or_else(|| {
            format!(
                "`{}` has no extent: its language provider does not report declaration extents",
                symbol.name
            )
        })?;
        let text = self.sources.text(&symbol.file)?;
        let range = LineIndex::new(&text)
            .range(extent)
            .filter(|r| !r.is_empty())
            .ok_or_else(|| format!("the extent of `{}` is outside its file", symbol.name))?;
        let start = text[..range.start].rfind('\n').map_or(0, |n| n + 1);
        let end = line_end_after(&text, range.end);
        let shares_line =
            !text[start..range.start].trim().is_empty() || code_follows(&text[range.end..end]);
        if shares_line {
            return Err(format!(
                "`{}` shares its lines with other code, so it cannot be moved on its own",
                symbol.name
            ));
        }
        let container = self.sources.containers(&symbol.file)?.at(range.start);
        Ok(Block {
            symbol: symbol.clone(),
            lines: start..end,
            container: container.open,
            group: container.group,
        })
    }

    fn relocate(&self, node: &Node, anchor: &Anchor) -> Result<Vec<TextEdit>, String> {
        let (Anchor::Before(target) | Anchor::After(target)) = anchor;
        let moved = self.block(node)?;
        let beside = self.block(target)?;
        let file = moved.symbol.file.clone();
        if beside.symbol.file != file {
            return Err("moving a declaration to another file is not supported".to_owned());
        }
        if moved.symbol.id == beside.symbol.id {
            return Err("a declaration cannot be moved next to itself".to_owned());
        }
        if moved.group || beside.group {
            return Err(format!(
                "`{}` is one of a parenthesized group, whose order is part of its meaning",
                moved.symbol.name
            ));
        }
        if moved.container != beside.container {
            return Err(format!(
                "`{}` and `{}` are not in the same block, so moving one next to the other would change what it belongs to",
                moved.symbol.name, beside.symbol.name
            ));
        }
        let text = self.sources.text(&file)?;
        let removal = with_blank(&text, moved.lines.clone());
        let eol = eol_of(&text);
        let mut block = text[moved.lines.clone()].to_owned();
        if !block.ends_with('\n') {
            block.push_str(eol);
        }
        let (at, inserted) = match anchor {
            Anchor::After(_) => {
                let lead = if text[..beside.lines.end].ends_with('\n') {
                    eol.to_owned()
                } else {
                    eol.repeat(2)
                };
                (beside.lines.end, format!("{lead}{block}"))
            }
            Anchor::Before(_) => (beside.lines.start, format!("{block}{eol}")),
        };
        if removal.start < at && at < removal.end {
            return Err("the target lies inside the moved declaration".to_owned());
        }
        Ok(vec![
            TextEdit::delete(file.clone(), removal),
            TextEdit::insert(file, at, inserted),
        ])
    }

    /// Puts the listed declarations in order within the places they occupy,
    /// one container at a time.
    fn reorder(&self, owner: &Owner, order: &[Node]) -> Result<Vec<TextEdit>, String> {
        let blocks: Vec<Block> = order
            .iter()
            .map(|n| self.block(n))
            .collect::<Result<_, _>>()?;
        let Some(first) = blocks.first() else {
            return Err("nothing to reorder".to_owned());
        };
        let file = first.symbol.file.clone();
        let member = |b: &Block| match owner {
            Owner::File(path) => b.symbol.file == *path,
            Owner::Symbol(id) => b.symbol.owner.as_ref() == Some(id),
        };
        if blocks.iter().any(|b| b.symbol.file != file || !member(b)) {
            return Err("the declarations to reorder do not share the named owner".to_owned());
        }
        let text = self.sources.text(&file)?;
        let mut containers: BTreeMap<Option<usize>, Vec<&Block>> = BTreeMap::new();
        for block in &blocks {
            containers.entry(block.container).or_default().push(block);
        }
        let mut edits = Vec::new();
        for group in containers.values() {
            if group.iter().any(|b| b.group) {
                continue;
            }
            let mut slots: Vec<&Block> = group.clone();
            slots.sort_by_key(|b| b.lines.start);
            if slots.windows(2).any(|w| w[0].lines.end > w[1].lines.start) {
                return Err("declarations to reorder overlap".to_owned());
            }
            for (slot, wanted) in slots.iter().zip(group.iter()) {
                if slot.symbol.id == wanted.symbol.id {
                    continue;
                }
                let mut body = text[wanted.lines.clone()].to_owned();
                if !body.ends_with('\n') {
                    body.push_str(eol_of(&text));
                }
                edits.push(TextEdit {
                    file: file.clone(),
                    range: slot.lines.clone(),
                    text: body,
                });
            }
        }
        Ok(edits)
    }

    fn rename(&self, id: &SymbolId, name: &str) -> Result<Vec<TextEdit>, String> {
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
        for site in self.project.sites(id) {
            if site.kind == EdgeKind::Contains {
                continue;
            }
            let text = self.sources.text(&site.file)?;
            let range = LineIndex::new(&text)
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

pub(crate) struct Lowerer<'a> {
    project: &'a Project,
    sources: &'a Sources<'a>,
    /// Whether the provider of a language declares `complete-references`.
    complete: &'a dyn Fn(&str) -> bool,
}

/// The end of the line that holds the end of a range: just past the newline
/// that closes it, or the end of the text. Never slices inside a character.
fn line_end_after(text: &str, end: usize) -> usize {
    if end > 0 && text.as_bytes()[end - 1] == b'\n' {
        return end;
    }
    text[end..].find('\n').map_or(text.len(), |n| end + n + 1)
}

/// Whether the rest of a line holds code: anything but whitespace, a line
/// comment, or one block comment that closes at the end of the line.
fn code_follows(rest: &str) -> bool {
    let rest = rest.trim();
    if rest.is_empty() || rest.starts_with("//") || rest.starts_with('#') {
        return false;
    }
    !(rest.starts_with("/*") && rest.find("*/") == rest.len().checked_sub(2))
}

/// `range` with the spaces and tabs before it included, for a deletion that
/// leaves other text on its line.
fn trimmed_back(text: &str, range: Range<usize>) -> Range<usize> {
    let kept = text[..range.start].trim_end_matches([' ', '\t']).len();
    kept..range.end
}

/// Where a symbol's name is first written as a whole word in its declaration,
/// not counting a Go receiver, which comes before the name.
fn name_in(declaration: &str, name: &str) -> Option<usize> {
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

#[cfg(test)]
mod tests;
