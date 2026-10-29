//! Where an expression, a call or a field lies in the host file. The parser
//! reports the byte range of a token with its last byte as the stop, and the
//! position of a call or a selection as that of its `(` or `.`.

use cel::common::ast::{CallExpr, IdedExpr};
use lighthouse_model::Span;

use crate::language::source::MappedText;

/// The last segment of a possibly qualified function name.
pub(super) fn base_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

pub(super) fn expr_span(program: &cel::Program, expr: &IdedExpr, mapped: &MappedText) -> Span {
    let (start, end) = expr_bounds(program, expr, mapped);
    mapped.span(start, end)
}

/// The name of the function a call names, which stands right before the `(`
/// the parser reports; the last occurrence of the name in the call otherwise.
pub(super) fn call_span(
    program: &cel::Program,
    expr: &IdedExpr,
    call: &CallExpr,
    mapped: &MappedText,
) -> Span {
    let name = base_name(&call.func_name);
    if let Some((start, _)) = program.source_info().offset_for(expr.id) {
        let end = (start as usize).min(mapped.text.len());
        if let Some(from) = end.checked_sub(name.len())
            && mapped.text.get(from..end) == Some(name)
        {
            return mapped.span(from, end);
        }
    }
    let (start, end) = expr_bounds(program, expr, mapped);
    mapped
        .text
        .get(start..end)
        .and_then(|call| call.rfind(name))
        .map_or_else(
            || mapped.span(start, end),
            |at| mapped.span(start + at, start + at + name.len()),
        )
}

/// The name of the field a selection reads, which follows the `.`, `.?` or
/// `?.` the parser reports; the last occurrence of the name in the selection
/// otherwise.
pub(super) fn field_span(
    program: &cel::Program,
    expr: &IdedExpr,
    field: &str,
    mapped: &MappedText,
) -> Span {
    let (start, end) = expr_bounds(program, expr, mapped);
    let after = mapped.text.get(start..).unwrap_or("");
    let dot = if after.starts_with(".?") || after.starts_with("?.") {
        Some(start + 2)
    } else if after.starts_with('.') {
        Some(start + 1)
    } else {
        None
    };
    dot.filter(|at| {
        mapped
            .text
            .get(*at..)
            .is_some_and(|rest| rest.starts_with(field))
    })
    .or_else(|| {
        let selection = mapped.text.get(start..end)?;
        selection.rfind(field).map(|at| start + at)
    })
    .map_or_else(
        || mapped.span(start, end),
        |at| mapped.span(at, at + field.len()),
    )
}

/// The half-open byte range of the text that `expr` covers, clamped to it; the
/// whole text where the parser reports none.
fn expr_bounds(program: &cel::Program, expr: &IdedExpr, mapped: &MappedText) -> (usize, usize) {
    let len = mapped.text.len();
    let (start, end) = program
        .source_info()
        .offset_for(expr.id)
        .map_or((0, len), |(start, stop)| {
            (start as usize, stop as usize + 1)
        });
    let start = start.min(len);
    (start, end.min(len).max(start))
}
