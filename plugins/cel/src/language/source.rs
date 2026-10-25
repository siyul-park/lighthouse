use lighthouse_model::{
    LineIndex, Span,
    ucm::{File, Fragment, Module, Symbol, SymbolId, SymbolKind, Visibility},
};
use lighthouse_plugin::Source;

pub(super) const MODULE: &str = "cel";

#[derive(Clone)]
pub(super) struct FileDraft {
    pub(super) file: File,
    pub(super) module: String,
    pub(super) fragment: Fragment,
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
            variables: Vec::new(),
            snippets: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub(super) struct VariableDecl {
    pub(super) doc_id: String,
    pub(super) name: String,
    pub(super) public: bool,
    pub(super) id: SymbolId,
    pub(super) span: Span,
    pub(super) extent: Span,
    pub(super) params: Vec<Parameter>,
}

impl VariableDecl {
    pub(super) fn symbol(&self, file: &File) -> Symbol {
        Symbol {
            id: self.id.clone(),
            kind: SymbolKind::Var,
            visibility: if self.public {
                Visibility::Public
            } else {
                Visibility::Private
            },
            owner: None,
            file: file.path.clone(),
            span: self.span,
            extent: Some(self.extent),
            doc: None,
            name: self.name.clone(),
            role: None,
            optional: false,
            type_ref: None,
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
            id: self.id.clone(),
            kind: SymbolKind::Var,
            visibility: Visibility::Private,
            owner: Some(self.owner.clone()),
            file: file.path.clone(),
            span: self.span,
            extent: Some(self.span),
            doc: None,
            name: self.name.clone(),
            role: None,
            optional: false,
            type_ref: None,
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
}

impl Snippet {
    pub(super) fn symbol(&self, file: &File) -> Symbol {
        let span = self
            .source
            .span(0, self.source.text.len(), &self.source.host);
        Symbol {
            id: self.id.clone(),
            kind: SymbolKind::Function,
            visibility: Visibility::Private,
            owner: None,
            file: file.path.clone(),
            span,
            extent: Some(span),
            doc: None,
            name: self.name.clone(),
            role: None,
            optional: false,
            type_ref: None,
        }
    }
}

#[derive(Clone)]
pub(super) struct MappedText {
    pub(super) text: String,
    pub(super) starts: Vec<usize>,
    pub(super) ends: Vec<usize>,
    pub(super) source_start: usize,
    pub(super) source_end: usize,
    pub(super) host: String,
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

    pub(super) fn span(&self, start: usize, end: usize, host: &str) -> Span {
        let start = start.min(self.text.len());
        let end = end.min(self.text.len()).max(start);
        if start == end {
            return span(host, self.source_start, self.source_start);
        }
        let from = self.starts.get(start).copied().unwrap_or(self.source_start);
        let to = self.ends.get(end - 1).copied().unwrap_or(self.source_end);
        span(host, from, to.max(from))
    }
}

pub(super) fn span(text: &str, start: usize, end: usize) -> Span {
    let index = LineIndex::new(text);
    let start = start.min(text.len());
    Span {
        start: index.position(start),
        end: index.position(end.min(text.len()).max(start)),
    }
}
