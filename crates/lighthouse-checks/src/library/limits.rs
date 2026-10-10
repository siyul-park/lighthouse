//! The limit functions of CEL expressions: `limit(n, max)` and
//! `counted(n, count, what)`.

use std::sync::Arc;

use cel::{
    Context, ExecutionError, Value,
    common::types::{CelMap, CelMapKey, CelString},
};

use super::{Builtin, arguments, member, text};

/// The key of a limit map that covers every role it does not name.
const FALLBACK_ROLE: &str = "default";

/// What a limit of `-1` says: there is none.
const NO_LIMIT: i64 = -1;

/// Adds `limit` and `counted` to the context.
pub(super) fn install(context: &mut Context<'static>) {
    context.add_function("limit", limit());
    context.add_function("counted", counted());
}

/// `limit(node, max)`: the integer a limit option gives the node's role, `-1`
/// for none. `max` is an integer (every role), or a map from a role or
/// `default` to an integer; a role the map leaves out takes its `default`, and
/// a map without one has no limit there.
fn limit() -> Builtin {
    Box::new(|ftx| {
        let [node, max] = arguments(ftx)?;
        let role = match member(node, "role", "limit")? {
            Some(value) => text(&Value::try_from(value)?),
            None => String::new(),
        };
        let Some(map) = max.downcast_ref::<CelMap>() else {
            return Ok(Value::Int(whole(&Value::try_from(max)?)));
        };
        let at = |key: &str| {
            map.inner()
                .get(&CelMapKey::String(CelString::from(key)))
                .map(|v| Value::try_from(v.as_ref()))
        };
        match at(&role).or_else(|| at(FALLBACK_ROLE)) {
            Some(found) => Ok(Value::Int(whole(&found?))),
            None => Ok(Value::Int(NO_LIMIT)),
        }
    })
}

/// A limit as an integer. The option schema admits nothing else, so the
/// other arms are unreachable for a validated option; they read as no limit.
fn whole(value: &Value) -> i64 {
    match value {
        Value::Int(i) => *i,
        Value::UInt(u) => i64::try_from(*u).unwrap_or(i64::MAX),
        _ => NO_LIMIT,
    }
}

/// `counted(node, n, what)`: how a limit finding says what a function has: a
/// constructor names the type it builds and needs `n` things, any other
/// function has them (`NewServer needs 9 parameters`, `parse has 9
/// parameters`).
fn counted() -> Builtin {
    Box::new(|ftx| {
        let [node, n, what] = arguments(ftx)?;
        let field = |name: &str| -> Result<String, ExecutionError> {
            Ok(match member(node, name, "counted")? {
                Some(value) => text(&Value::try_from(value)?),
                None => String::new(),
            })
        };
        // The count is an integer by construction of the callers.
        let n = whole(&Value::try_from(n)?).max(0);
        let what = text(&Value::try_from(what)?);
        let (subject, verb) = if field("role")? == "constructor" {
            (field("built")?, "needs")
        } else {
            (field("name")?, "has")
        };
        Ok(Value::String(Arc::new(format!(
            "{subject} {verb} {n} {what}"
        ))))
    })
}
