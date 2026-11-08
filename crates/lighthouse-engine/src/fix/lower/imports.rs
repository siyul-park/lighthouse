//! The imports of a file, as its provider reports them: a declaration is never
//! put above a module-level import it followed, which would use a name before
//! it is bound. An import of a declaration's body (a function-local one) is no
//! module-level import.

use std::{ops::Range, path::Path};

use lighthouse_model::{
    LineIndex,
    ucm::{EdgeKind, Node},
};

use super::Lowerer;

impl Lowerer<'_> {
    /// Whether none of the `placed` declarations, each as the offset it moves
    /// from, the offset it is put at and its module, ends above an import of
    /// its module that it was below.
    pub(super) fn keeps_imports(
        &self,
        file: &Path,
        text: &str,
        placed: &[(usize, usize, String)],
    ) -> Result<(), String> {
        if placed.is_empty() {
            return Ok(());
        }
        let imports = self.imports(file, text);
        for (from, at, module) in placed {
            let above = imports
                .iter()
                .find(|(import, of)| of == module && import.end <= *from && *at <= import.start);
            if let Some((import, _)) = above {
                let line = text[..import.start].matches('\n').count() + 1;
                return Err(format!(
                    "the move would put a declaration above the imports of the file: `{}` on line {line}",
                    text[import.clone()]
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .trim()
                ));
            }
        }
        Ok(())
    }

    /// The whole lines of the module-level imports of `file`: the sites of its
    /// `imports` edges from a module that no declaration of the file holds, with
    /// the module each binds names in.
    pub(super) fn imports(&self, file: &Path, text: &str) -> Vec<(Range<usize>, String)> {
        let index = LineIndex::new(text);
        let held: Vec<Range<usize>> = self
            .project
            .symbols_in(file)
            .filter_map(|s| index.range(self.project.place(&s.id)?.extent?))
            .collect();
        self.project
            .edges()
            .enumerate()
            .filter(|(at, e)| e.kind == EdgeKind::Imports && self.project.origin(*at) == Some(file))
            .filter_map(|(_, e)| match &e.from {
                Node::Module(module) => Some((index.range(e.site?)?, module.clone())),
                Node::Symbol(_) => None,
            })
            .filter(|(site, _)| {
                !held
                    .iter()
                    .any(|h| h.start <= site.start && site.end <= h.end)
            })
            .map(|(site, module)| {
                let start = text[..site.start].rfind('\n').map_or(0, |n| n + 1);
                let end = text[site.end..]
                    .find('\n')
                    .map_or(text.len(), |n| site.end + n + 1);
                (start..end, module)
            })
            .collect()
    }
}
