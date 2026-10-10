//! The facts about the types a declaration is written with (`signature` of a
//! function, `type_ref` of a field) and the body events of a function, which a
//! `select: event` check takes one finding each.

use lighthouse_model::{Event, FunctionSummary, Span, Symbol, TypeRef};
use serde_json::{Map, Value, json};

use super::Builder;
use crate::eval::{Fact, cel_fact};

/// One event of a function, ready to be judged.
pub(crate) struct EventFact {
    pub fact: Fact,
    pub span: Span,
    /// What tells this event from the others of the function from one run to
    /// the next: its kind and its place among the events of that kind.
    pub key: String,
}

impl Builder<'_> {
    /// `signature` and `type_ref`, for the expressions that mention them.
    pub(super) fn api_facts(
        &self,
        map: &mut Map<String, Value>,
        symbol: &Symbol,
        summary: Option<&FunctionSummary>,
    ) {
        if self.needs.fact("signature") {
            let signature = summary.map(|s| &s.signature);
            let list = |types: Option<&[TypeRef]>| {
                types
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .map(|(at, t)| type_ref(Some(t), at))
                    .collect::<Vec<_>>()
            };
            map.insert(
                "signature".to_owned(),
                json!({
                    "params": list(signature.map(|s| s.params.as_slice())),
                    "results": list(signature.map(|s| s.results.as_slice())),
                }),
            );
        }
        if self.needs.fact("type_ref") {
            map.insert("type_ref".to_owned(), type_ref(symbol.type_ref.as_ref(), 0));
        }
    }

    /// The events of a function, each as `event` sees it: `kind`, `detail`,
    /// the place (`line`, `end_line`, `col`, `end_col`) and the function it is in
    /// (`func`).
    pub(crate) fn event_facts(&self, symbol: &Symbol) -> Vec<EventFact> {
        let Some(summary) = self.project.function(&symbol.id) else {
            return Vec::new();
        };
        if summary.events.is_empty() {
            return Vec::new();
        }
        let func = self.symbol(symbol);
        let mut seen: Vec<((&str, &str), u32)> = Vec::new();
        summary
            .events
            .iter()
            .map(|event| {
                let kind = event.kind.as_str();
                let detail = event.detail.as_deref().unwrap_or_default();
                let ordinal = ordinal(&mut seen, (kind, detail));
                EventFact {
                    fact: cel_fact(&event_value(event, &func)),
                    span: event.span,
                    key: format!("{}#{kind}#{detail}#{ordinal}", symbol.id.as_str()),
                }
            })
            .collect()
    }
}

/// How many events of the same kind and detail came before this one, so that
/// a different event in front of it does not move its identity.
fn ordinal<'a>(seen: &mut Vec<((&'a str, &'a str), u32)>, id: (&'a str, &'a str)) -> u32 {
    match seen.iter_mut().find(|(k, _)| *k == id) {
        Some((_, n)) => {
            *n += 1;
            *n
        }
        None => {
            seen.push((id, 0));
            0
        }
    }
}

fn event_value(event: &Event, func: &Value) -> Value {
    json!({
        "kind": event.kind.as_str(),
        "detail": event.detail.clone().unwrap_or_default(),
        "line": event.span.start.line,
        "end_line": event.span.end.line,
        "col": event.span.start.col,
        "end_col": event.span.end.col,
        "func": func,
    })
}

/// A type reference; `known` is false for a field the provider gave no type.
fn type_ref(t: Option<&TypeRef>, index: usize) -> Value {
    json!({
        "index": index,
        "known": t.is_some(),
        "text": t.map_or("", |t| t.text.as_str()),
        "symbol": t.and_then(|t| t.symbol.as_deref()).unwrap_or_default(),
        "exported": t.and_then(|t| t.exported).unwrap_or(false),
    })
}
