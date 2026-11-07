//! Lowering: moves, deletes, reorders, renames and the refusals around them.

use std::fs;

use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, File, Fragment, Place, Project, Resolution, SymbolKind, Target, Visibility,
    },
};

use super::{rename::name_in, *};
use crate::fix::edits::apply;

mod deletes;
mod guards;
mod load_order;
mod moves;
mod rename;
mod reorder;
mod scope;
mod spacing;

const FILE: &str = "a.src";

const THREE: &str = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";

fn span(from: (u32, u32), to: (u32, u32)) -> Span {
    Span {
        start: Position {
            line: from.0,
            col: from.1,
        },
        end: Position {
            line: to.0,
            col: to.1,
        },
    }
}

/// A declaration that fills whole lines `from..=to` of `text`.
fn symbol(text: &str, name: &str, from: u32, to: u32) -> (Symbol, Place) {
    let last = text.lines().nth(to as usize - 1).unwrap_or_default();
    let extent = span((from, 1), (to, u32::try_from(last.len()).unwrap() + 1));
    let id = SymbolId::new("m", &[], name, SymbolKind::Function);
    let place = Place {
        extent: Some(extent),
        ..Place::new(id.clone(), extent)
    };
    let symbol = Symbol {
        id,
        kind: SymbolKind::Function,
        visibility: Visibility::Private,
        owner: None,
        file: FILE.into(),
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
        implementation: false,
    };
    (symbol, place)
}

fn id(name: &str) -> Node {
    Node::Symbol(SymbolId::new("m", &[], name, SymbolKind::Function))
}

struct Case {
    dir: tempfile::TempDir,
    project: Project,
}

fn case(text: &str, symbols: &[(&str, u32, u32)], edges: Vec<Edge>) -> Case {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), text).unwrap();
    let declared: Vec<(Symbol, Place)> = symbols
        .iter()
        .map(|(name, from, to)| symbol(text, name, *from, *to))
        .collect();
    let project = Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "src".to_owned(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        symbols: declared.iter().map(|(symbol, _)| symbol.clone()).collect(),
        places: declared.into_iter().map(|(_, place)| place).collect(),
        edges,
        ..Fragment::default()
    }]);
    Case { dir, project }
}

impl Case {
    /// The file after lowering and applying `ops`, or why they were refused.
    fn run(&self, ops: &[EditOp]) -> Result<String, String> {
        let overlays = Overlays::new();
        let sources = Sources::new(self.dir.path(), &self.project, &overlays);
        let complete = |_: &str| true;
        let lowerer = Lowerer::new(&self.project, &sources, &complete);
        let mut edits = Vec::new();
        for op in ops {
            edits.extend(lowerer.lower(op)?);
        }
        if crate::fix::edits::self_overlap(&edits) {
            return Err("the edits of the fix overlap each other".to_owned());
        }
        let text = sources.text(Path::new(FILE))?;
        apply(&text, &edits.iter().collect::<Vec<_>>())
    }
}

fn after(node: &str, anchor: &str) -> EditOp {
    EditOp::Move {
        node: id(node),
        anchor: Anchor::After(id(anchor)),
    }
}
