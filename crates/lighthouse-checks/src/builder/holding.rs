//! The holding graph of the types of a file: a type holds another when one of
//! its fields has the other as its type, pointer, slice, map or generic
//! argument included. `holders` are the types that hold a type, `hold_cycle`
//! the types in its strongly connected component of that graph, as
//! `module.cycle` is for imports.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    rc::Rc,
};

use lighthouse_model::{
    graph::Graph,
    ucm::{Symbol, SymbolKind},
};
use serde_json::{Map, Value, json};

use super::Builder;
use crate::{layout::owner_key, library::Needs};

/// What the holding graph of one file says of each of its types.
#[derive(Default)]
pub(super) struct Holding {
    holders: BTreeMap<String, BTreeSet<String>>,
    cycles: BTreeMap<String, Vec<String>>,
}

impl Builder<'_> {
    /// The facts of the holding graph of `symbol`, each when `needs` asks.
    pub(super) fn holding_facts(
        &self,
        map: &mut Map<String, Value>,
        symbol: &Symbol,
        needs: &Needs,
    ) {
        if symbol.kind != SymbolKind::Type {
            return;
        }
        if needs.field("holders") {
            map.insert("holders".to_owned(), json!(self.holders(symbol)));
        }
        if needs.field("hold_cycle") {
            map.insert("hold_cycle".to_owned(), json!(self.hold_cycle(symbol)));
        }
    }

    /// The other types of the file that hold `ty`, ascending.
    fn holders(&self, ty: &Symbol) -> Vec<String> {
        let holding = self.holding_graph(&ty.file);
        let held = holding.holders.get(ty.id.as_str());
        held.map(|holders| holders.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The types in the strongly connected component of `ty`, itself included
    /// and ascending; empty when it is on no cycle.
    fn hold_cycle(&self, ty: &Symbol) -> Vec<String> {
        let holding = self.holding_graph(&ty.file);
        holding
            .cycles
            .get(ty.id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// The holding graph of `file`, found once.
    fn holding_graph(&self, file: &Path) -> Rc<Holding> {
        let mut held = self.holdings.borrow_mut();
        let holding = held
            .entry(file.to_path_buf())
            .or_insert_with(|| Rc::new(self.build_holding(file)));
        Rc::clone(holding)
    }

    /// For each type of `file`: the other types of the file that hold it
    /// (ascending), and the types in its strongly connected component of the
    /// holding graph, itself included and ascending, empty when it is on no
    /// cycle. A type that holds itself is on a cycle of one.
    fn build_holding(&self, file: &Path) -> Holding {
        let edges = self.holding_edges(file);
        let ids: Vec<&String> = edges
            .iter()
            .flat_map(|(from, to)| [from, to])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let at = |id: &String| u32::try_from(ids.binary_search(&id).ok()?).ok();
        let graph = Graph::new(
            ids.len(),
            edges
                .iter()
                .filter_map(|(from, to)| Some((at(from)?, at(to)?))),
        );
        let (component, _) = graph.components();
        let mut members: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for (v, id) in ids.iter().enumerate() {
            members.entry(component[v]).or_default().push((*id).clone());
        }
        let mut holding = Holding::default();
        for (holder, held) in edges.iter().filter(|(from, to)| from != to) {
            holding
                .holders
                .entry(held.clone())
                .or_default()
                .insert(holder.clone());
        }
        let self_held: BTreeSet<&String> = edges
            .iter()
            .filter(|(from, to)| from == to)
            .map(|(from, _)| from)
            .collect();
        for (v, id) in ids.iter().enumerate() {
            let group = &members[&component[v]];
            if group.len() > 1 || self_held.contains(id) {
                holding.cycles.insert((*id).clone(), group.clone());
            }
        }
        holding
    }

    /// The `(holder, held)` ids of the types of `file`: for each field of the
    /// file, the types of the file its type names (its own type included).
    fn holding_edges(&self, file: &Path) -> Vec<(String, String)> {
        let symbols: Vec<&Symbol> = self.project.symbols_in(file).collect();
        let types: Vec<&Symbol> = symbols
            .iter()
            .copied()
            .filter(|s| s.kind == SymbolKind::Type)
            .collect();
        let mut by_name: BTreeMap<&str, Vec<&Symbol>> = BTreeMap::new();
        for ty in &types {
            let id = ty.id.as_str().split('#').next().unwrap_or_default();
            by_name.entry(id).or_default().push(ty);
        }
        let mut edges = Vec::new();
        for field in symbols.iter().filter(|s| s.kind == SymbolKind::Field) {
            let Some(holder) = owner_key(field) else {
                continue;
            };
            let Some(type_ref) = &field.type_ref else {
                continue;
            };
            let held: Vec<&Symbol> = match type_ref.symbol.as_deref() {
                Some(named) if !named.is_empty() => by_name.get(named).cloned().unwrap_or_default(),
                _ => types
                    .iter()
                    .copied()
                    .filter(|ty| self.named_in(&type_ref.text, ty))
                    .collect(),
            };
            edges.extend(
                held.into_iter()
                    .map(|ty| (holder.clone(), ty.id.as_str().to_owned())),
            );
        }
        edges
    }

    /// Whether `text` names the type `held`: qualified by its module in Go,
    /// as a word in any other language.
    fn named_in(&self, text: &str, held: &Symbol) -> bool {
        let go = self.project.language_of(held) == Some("go");
        let qualifier = match (go, held.id.module()) {
            (false, _) => String::new(),
            (true, ".") => ".".to_owned(),
            (true, module) => format!("/{module}."),
        };
        let needle = format!("{qualifier}{}", held.name);
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        text.match_indices(&needle).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + needle.len()..].chars().next();
            (go || !before.is_some_and(word)) && !after.is_some_and(word)
        })
    }
}
