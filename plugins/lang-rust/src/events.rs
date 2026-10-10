//! The body events of a function that rules may flag, as clippy's restriction
//! lints see them: the panicking macros and `unwrap`/`expect` calls. They are
//! syntactic: a method named `unwrap` counts on any receiver.

use lighthouse_protocol::{Event, EventKind};
use syn::{
    Block, ExprMethodCall, ItemFn, ItemImpl, Macro,
    visit::{self, Visit},
};

use crate::{
    macros,
    tree::SourceFile,
    util::{ident_span, span_of},
};

/// The macros that panic or mark code that must not run.
const PANICKING: [&str; 4] = ["panic", "unreachable", "todo", "unimplemented"];

/// The methods that panic on `None`, `Err` and `Ok`.
const UNWRAPPING: [&str; 4] = ["unwrap", "expect", "unwrap_err", "expect_err"];

struct Collector<'a> {
    src: &'a SourceFile,
    events: Vec<Event>,
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item_fn(&mut self, _: &'ast ItemFn) {}

    fn visit_item_impl(&mut self, _: &'ast ItemImpl) {}

    fn visit_macro(&mut self, mac: &'ast Macro) {
        let name = mac.path.segments.last().map(|s| s.ident.to_string());
        if let Some(name) = name.filter(|n| PANICKING.contains(&n.as_str())) {
            self.events.push(Event {
                kind: EventKind::Panic,
                span: span_of(self.src, mac),
                detail: Some(name),
            });
        }
        for argument in macros::arguments(mac).unwrap_or_default() {
            self.visit_expr(&argument);
        }
    }

    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        let name = call.method.to_string();
        if UNWRAPPING.contains(&name.as_str()) {
            self.events.push(Event {
                kind: EventKind::Unwrap,
                span: ident_span(self.src, &call.method),
                detail: Some(name),
            });
        }
        visit::visit_expr_method_call(self, call);
    }
}

/// The events of a function body in source order. Functions declared inside
/// the body have summaries of their own and are not read here.
pub fn of(src: &SourceFile, block: &Block) -> Vec<Event> {
    let mut found = Collector {
        src,
        events: Vec::new(),
    };
    found.visit_block(block);
    let mut events = found.events;
    events.sort_by_key(|e| (e.span.start.line, e.span.start.col));
    events
}
