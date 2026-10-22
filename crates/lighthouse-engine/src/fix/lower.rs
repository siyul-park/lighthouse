//! Lowering: edit operations over code-model nodes become text edits, using
//! the extents and sites language providers report. A Move is a deletion of
//! one block of whole lines and an insertion of the same lines elsewhere, so
//! comments and formatting travel with the declaration as written.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use lighthouse_model::{
    LineIndex, Position, Span, eol_of,
    ucm::{Anchor, EditOp, Node, Owner, Project, Symbol, SymbolId},
};

use crate::Overlays;

use super::{
    edits::{TextEdit, whole_lines, with_blank},
    scan::Containers,
};

mod rename;

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
/// fills, the bracket it sits in and, for one of a parenthesized group, where
/// that closes.
struct Block {
    symbol: Symbol,
    lines: Range<usize>,
    container: Option<usize>,
    group: Option<usize>,
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
            EditOp::Delete { node } => self.delete(node),
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

    /// Removes a declaration. One spec of a parenthesized `const` group that a
    /// later spec depends on for its value (`iota`, or the expression it
    /// repeats) keeps its place as `_`, so that the specs after it keep their
    /// values; a spec that a later spec names cannot be removed.
    fn delete(&self, node: &Node) -> Result<Vec<TextEdit>, String> {
        let block = self.block(node)?;
        let file = block.symbol.file.clone();
        let text = self.sources.text(&file)?;
        if let (Some(open), Some(close)) = (block.container, block.group)
            && let Some(later) = const_later(&text, open, close, &block)
        {
            match later {
                Later::Independent => {}
                Later::Positional => return self.blank(&block, &text),
                Later::Named => {
                    return Err(format!(
                        "`{}` is named by a spec after it in its `const` group",
                        block.symbol.name
                    ));
                }
            }
        }
        let range = with_blank(&text, block.lines.clone());
        Ok(vec![TextEdit::delete(file, range)])
    }

    /// The edits that leave the spec of `block` in its place with the blank
    /// identifier for a name, and its leading comments gone.
    fn blank(&self, block: &Block, text: &str) -> Result<Vec<TextEdit>, String> {
        let symbol = &block.symbol;
        let name = LineIndex::new(text)
            .range(Span {
                start: symbol.span.start,
                end: Position {
                    line: symbol.span.start.line,
                    col: symbol.span.start.col + u32::try_from(symbol.name.len()).unwrap_or(0),
                },
            })
            .filter(|range| text.get(range.clone()) == Some(symbol.name.as_str()))
            .ok_or_else(|| {
                format!(
                    "the name of `{}` is not where the provider says",
                    symbol.name
                )
            })?;
        let mut edits = Vec::new();
        if block.lines.start < name.start {
            let own_line = text[..name.start].rfind('\n').map_or(0, |n| n + 1);
            if block.lines.start < own_line {
                edits.push(TextEdit::delete(
                    symbol.file.clone(),
                    block.lines.start..own_line,
                ));
            }
        }
        edits.push(TextEdit {
            file: symbol.file.clone(),
            range: name,
            text: "_".to_owned(),
        });
        Ok(edits)
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
        if moved.group.is_some() || beside.group.is_some() {
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
        let blocks = self.units(blocks)?;
        let mut containers: BTreeMap<Option<usize>, Vec<&Block>> = BTreeMap::new();
        for block in &blocks {
            containers.entry(block.container).or_default().push(block);
        }
        let mut edits = Vec::new();
        for group in containers.values() {
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

    /// `blocks` as the reorder moves them. A declaration directly inside a `(`
    /// container is one of a grouped declaration, as Go's `const ( ... )`
    /// (providers of other languages report no symbol there). The specs of
    /// one are a single block, the declaration from its first line, with the
    /// comments that lead it, to its closing paren, put where its first spec
    /// is listed. The reorder is declined when the order would split it: its
    /// specs are not listed together and in file order, or one is not listed.
    fn units(&self, blocks: Vec<Block>) -> Result<Vec<Block>, String> {
        let listed: BTreeSet<SymbolId> = blocks.iter().map(|b| b.symbol.id.clone()).collect();
        let mut units = Vec::new();
        let mut first: BTreeMap<usize, String> = BTreeMap::new();
        let mut previous: Option<(usize, usize, String)> = None;
        for block in blocks {
            let Some(open) = block.group.and(block.container) else {
                units.push(block);
                previous = None;
                continue;
            };
            let last = previous.replace((open, block.lines.start, block.symbol.name.clone()));
            let Some(head) = first.get(&open) else {
                first.insert(open, block.symbol.name.clone());
                units.push(self.unit(block, &listed)?);
                continue;
            };
            match last {
                Some((p, start, _)) if p == open && start < block.lines.start => {}
                Some((p, _, before)) if p == open => {
                    return Err(format!(
                        "`{}` and `{before}` of one grouped declaration are ordered against their file order; split it",
                        block.symbol.name
                    ));
                }
                _ => {
                    return Err(format!(
                        "`{}` is ordered apart from `{head}`, the first of its grouped declaration; split it",
                        block.symbol.name
                    ));
                }
            }
        }
        Ok(units)
    }

    /// The grouped declaration `spec` is one of, in place of the spec, if it
    /// is one. Every symbol it holds must be `listed`: one the order does not
    /// mention would move with the unit unasked.
    fn unit(&self, spec: Block, listed: &BTreeSet<SymbolId>) -> Result<Block, String> {
        let (Some(open), Some(close)) = (spec.container, spec.group) else {
            return Ok(spec);
        };
        let file = &spec.symbol.file;
        let text = self.sources.text(file)?;
        let index = LineIndex::new(&text);
        let loose = self.project.symbols_in(file).find(|s| {
            s.owner.is_none()
                && !listed.contains(&s.id)
                && index
                    .range(s.span)
                    .is_some_and(|r| open < r.start && r.start < close)
        });
        if let Some(loose) = loose {
            return Err(format!(
                "`{}` of a grouped declaration is not ordered; it stays in place",
                loose.name
            ));
        }
        let line = text[..open].rfind('\n').map_or(0, |n| n + 1);
        let keyword = line + text[line..].len() - text[line..].trim_start().len();
        let lines = lines_of(&text, &(with_leading_comments(&text, keyword)..close + 1))
            .ok_or_else(|| {
                format!(
                    "`{}` is one of a grouped declaration that shares its lines with other code",
                    spec.symbol.name
                )
            })?;
        let container = self.sources.containers(file)?.at(open).open;
        Ok(Block {
            lines,
            container,
            group: None,
            ..spec
        })
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
        let lines = lines_of(&text, &range).ok_or_else(|| {
            format!(
                "`{}` shares its lines with other code, so it cannot be moved on its own",
                symbol.name
            )
        })?;
        let container = self.sources.containers(&symbol.file)?.at(range.start);
        Ok(Block {
            symbol: symbol.clone(),
            lines,
            container: container.open,
            group: container.group,
        })
    }
}

pub(crate) struct Lowerer<'a> {
    project: &'a Project,
    sources: &'a Sources<'a>,
    /// Whether the provider of a language declares `complete-references`.
    complete: &'a dyn Fn(&str) -> bool,
}

/// The whole lines `range` fills, or `None` when other code shares them.
fn lines_of(text: &str, range: &Range<usize>) -> Option<Range<usize>> {
    let start = text[..range.start].rfind('\n').map_or(0, |n| n + 1);
    let end = line_end_after(text, range.end);
    let shares = !text[start..range.start].trim().is_empty() || code_follows(&text[range.end..end]);
    (!shares).then_some(start..end)
}

/// `start`, moved up over the comments that lead its line: `//` lines and
/// `/* */` blocks that begin their lines.
fn with_leading_comments(text: &str, mut start: usize) -> usize {
    while start > 0 {
        let above = text[..start - 1].rfind('\n').map_or(0, |n| n + 1);
        let line = text[above..start].trim();
        let opens = if line.starts_with("//") {
            Some(above)
        } else if line.ends_with("*/") {
            text[..start]
                .rfind("/*")
                .map(|at| text[..at].rfind('\n').map_or(0, |n| n + 1))
                .filter(|&l| text[l..].trim_start().starts_with("/*"))
        } else {
            None
        };
        let Some(opens) = opens else { break };
        start = opens;
    }
    start
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

#[cfg(test)]
mod tests;

/// What the specs of a `const` group after one that is removed need of it.
enum Later {
    /// Nothing: they give their own values.
    Independent,
    /// Its place, for the `iota` they count with or the expression they repeat.
    Positional,
    /// Its name, which they spell.
    Named,
}

/// What the specs after the spec of `removed` in its `const` group need of it;
/// `None` for a group that is not a `const` group or has no spec after it. A
/// spec after it depends on its place when `iota` is in the expression it
/// gives or repeats, and on the spec itself when it repeats the expression
/// that one gives.
fn const_later(text: &str, open: usize, close: usize, removed: &Block) -> Option<Later> {
    if !text[..open].trim_end().ends_with("const") {
        return None;
    }
    let specs = |from: usize, to: usize| -> Vec<&str> {
        text.get(from..to)
            .into_iter()
            .flat_map(str::lines)
            .map(|line| line.split("//").next().unwrap_or_default().trim())
            .filter(|line| !line.is_empty() && !line.starts_with("/*") && !line.starts_with('*'))
            .collect()
    };
    let before = specs(open + 1, removed.lines.start);
    let own = specs(removed.lines.start, removed.lines.end);
    let later = specs(removed.lines.end, close);
    if later.is_empty() {
        return None;
    }
    if later
        .iter()
        .any(|spec| has_word(spec, &removed.symbol.name))
    {
        return Some(Later::Named);
    }
    // The expression a spec gives or repeats, with whether it has `iota` and
    // whether the removed spec gives it.
    let expression = |spec: &str| spec.split_once('=').map(|(_, rhs)| has_word(rhs, "iota"));
    let mut governing = (false, false);
    for spec in &before {
        if let Some(iota) = expression(spec) {
            governing = (iota, false);
        }
    }
    if let Some(iota) = own.first().and_then(|spec| expression(spec)) {
        governing = (iota, true);
    }
    for spec in &later {
        match expression(spec) {
            Some(iota) => governing = (iota, false),
            None if governing.1 => return Some(Later::Positional),
            None => {}
        }
        if governing.0 {
            return Some(Later::Positional);
        }
    }
    Some(Later::Independent)
}

/// Whether `word` occurs in `text` as a whole word.
fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|found| found == word)
}
