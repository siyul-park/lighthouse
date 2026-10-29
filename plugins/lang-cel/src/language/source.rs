use std::sync::Arc;

use lighthouse_model::{
    LineIndex, Span,
    ucm::{File, Fragment, Module, Symbol, SymbolId, SymbolKind, Visibility},
};
use lighthouse_plugin::Source;

pub(super) const MODULE: &str = "cel";

/// The text of a host file with the offsets of its lines, which every
/// expression of the file shares.
pub(super) struct Host {
    text: String,
    starts: Vec<usize>,
}

impl Host {
    pub(super) fn new(text: &str) -> Arc<Self> {
        Arc::new(Self {
            text: text.to_owned(),
            starts: LineIndex::starts_of(text),
        })
    }

    /// The span of the byte range `start..end`, clamped to the text.
    pub(super) fn span(&self, start: usize, end: usize) -> Span {
        let index = LineIndex::over(&self.text, &self.starts);
        let start = start.min(self.text.len());
        Span {
            start: index.position(start),
            end: index.position(end.min(self.text.len()).max(start)),
        }
    }
}

#[derive(Clone)]
pub(super) struct FileDraft {
    pub(super) file: File,
    pub(super) module: String,
    pub(super) fragment: Fragment,
    pub(super) host: Arc<Host>,
    pub(super) variables: Vec<VariableDecl>,
    pub(super) snippets: Vec<Snippet>,
}

impl FileDraft {
    pub(super) fn empty(source: &Source) -> Self {
        let module = MODULE.to_owned();
        Self {
            file: source.file.clone(),
            module: module.clone(),
            fragment: Fragment {
                files: vec![source.file.clone()],
                modules: vec![Module {
                    path: module.clone(),
                    name: Some(MODULE.to_owned()),
                    test_of: None,
                }],
                ..Fragment::default()
            },
            host: Host::new(source.text),
            variables: Vec::new(),
            snippets: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub(super) struct VariableDecl {
    pub(super) doc_id: String,
    pub(super) name: String,
    pub(super) shared: bool,
    pub(super) id: SymbolId,
    pub(super) span: Span,
    pub(super) extent: Span,
    pub(super) params: Vec<Parameter>,
}

impl VariableDecl {
    pub(super) fn symbol(&self, file: &File) -> Symbol {
        Symbol {
            extent: Some(self.extent),
            ..Symbol::new(
                self.id.clone(),
                SymbolKind::Var,
                Visibility::Private,
                file.path.clone(),
                self.span,
                &self.name,
            )
        }
    }
}

#[derive(Clone)]
pub(super) struct Parameter {
    pub(super) name: String,
    pub(super) id: SymbolId,
    pub(super) owner: SymbolId,
    pub(super) span: Span,
}

impl Parameter {
    pub(super) fn symbol(&self, file: &File) -> Symbol {
        Symbol {
            owner: Some(self.owner.clone()),
            extent: Some(self.span),
            ..Symbol::new(
                self.id.clone(),
                SymbolKind::Var,
                Visibility::Private,
                file.path.clone(),
                self.span,
                &self.name,
            )
        }
    }
}

#[derive(Clone)]
pub(super) struct Snippet {
    pub(super) doc_id: String,
    pub(super) id: SymbolId,
    pub(super) source: MappedText,
    pub(super) name: String,
    pub(super) params: Vec<Parameter>,
    pub(super) declared: bool,
    pub(super) summarized: bool,
}

impl Snippet {
    pub(super) fn symbol(&self, file: &File) -> Symbol {
        let span = self.source.span(0, self.source.text.len());
        Symbol {
            extent: Some(span),
            ..Symbol::new(
                self.id.clone(),
                SymbolKind::Function,
                Visibility::Public,
                file.path.clone(),
                span,
                &self.name,
            )
        }
    }
}

/// The text of one expression, and where each of its bytes lies in the host
/// file.
#[derive(Clone)]
pub(super) struct MappedText {
    pub(super) text: String,
    pub(super) starts: Vec<usize>,
    pub(super) ends: Vec<usize>,
    pub(super) source_start: usize,
    pub(super) source_end: usize,
    pub(super) host: Arc<Host>,
}

impl MappedText {
    pub(super) fn slice(&self, start: usize, end: usize) -> Self {
        let start = start.min(self.text.len());
        let end = end.min(self.text.len()).max(start);
        Self {
            text: self.text[start..end].to_owned(),
            starts: self.starts[start..end].to_vec(),
            ends: self.ends[start..end].to_vec(),
            source_start: self.starts.get(start).copied().unwrap_or(self.source_start),
            source_end: self
                .ends
                .get(end.saturating_sub(1))
                .copied()
                .unwrap_or(self.source_end),
            host: self.host.clone(),
        }
    }

    /// The span in the host file of the bytes `start..end` of the text.
    pub(super) fn span(&self, start: usize, end: usize) -> Span {
        let start = start.min(self.text.len());
        let end = end.min(self.text.len()).max(start);
        if start == end {
            return self.host.span(self.source_start, self.source_start);
        }
        let from = self.starts.get(start).copied().unwrap_or(self.source_start);
        let to = self.ends.get(end - 1).copied().unwrap_or(self.source_end);
        self.host.span(from, to.max(from))
    }
}
