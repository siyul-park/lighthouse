//! The functions a CEL expression can call, as symbols of the code model: the
//! standard library of CEL with its extensions, and the libraries of the
//! plugins. The tables list what the `cel` crate ships, which it does not let
//! a caller enumerate; a test holds their namespaces to those of the
//! environment.

use std::{collections::BTreeMap, path::PathBuf};

use cel::common::ast::{CallExpr, Expr};
use lighthouse_model::{
    Position, Span,
    ucm::{Symbol, SymbolId, SymbolKind, Visibility},
};
use lighthouse_plugin::{CelLibraryManifest, Function};

use super::spans::base_name;

const GLOBALS: &[&str] = &[
    "bool",
    "bytes",
    "double",
    "duration",
    "int",
    "size",
    "string",
    "timestamp",
    "type",
    "uint",
    "has",
];
const MEMBERS: &[&str] = &[
    "charAt",
    "contains",
    "distinct",
    "endsWith",
    "flatten",
    "format",
    "getDate",
    "getDayOfMonth",
    "getDayOfWeek",
    "getDayOfYear",
    "getFullYear",
    "getHours",
    "getMilliseconds",
    "getMinutes",
    "getMonth",
    "getSeconds",
    "indexOf",
    "join",
    "lastIndexOf",
    "lowerAscii",
    "matches",
    "or",
    "orValue",
    "replace",
    "reverse",
    "slice",
    "sort",
    "sortBy",
    "size",
    "split",
    "startsWith",
    "substring",
    "trim",
    "upperAscii",
    "value",
    "hasValue",
];
const OPTIONALS: &[&str] = &["of", "none", "ofNonZeroValue"];
const NAMESPACE_FUNCTIONS: &[(&str, &[&str])] = &[
    ("base64", &["encode", "decode"]),
    ("lists", &["range"]),
    ("math", MATH),
    ("optional", OPTIONALS),
    ("strings", &["quote"]),
];

const MATH: &[&str] = &[
    "abs",
    "bitAnd",
    "bitNot",
    "bitOr",
    "bitShiftLeft",
    "bitShiftRight",
    "bitXor",
    "ceil",
    "floor",
    "isFinite",
    "isInf",
    "isNaN",
    "round",
    "sign",
    "sqrt",
    "trunc",
    "greatest",
    "least",
];

/// The namespaces of the standard functions, with the names in each.
pub(super) fn namespaces() -> impl Iterator<Item = &'static str> {
    NAMESPACE_FUNCTIONS.iter().map(|(namespace, _)| *namespace)
}

/// The identifier a call is made on: `strings` of `strings.quote(x)`.
fn call_namespace(call: &CallExpr) -> Option<&str> {
    call.target.as_ref().and_then(|target| match &target.expr {
        Expr::Ident(name) => Some(name.as_str()),
        _ => None,
    })
}

/// Every function the expressions can call, by symbol: the standard ones and
/// those of `libraries`.
pub(in crate::language) fn function_catalog(
    libraries: &[CelLibraryManifest],
) -> BTreeMap<SymbolId, Symbol> {
    let mut symbols = BTreeMap::new();
    let mut add = |symbol: Symbol| {
        symbols.entry(symbol.id.clone()).or_insert(symbol);
    };
    for name in GLOBALS {
        add(standard_symbol("cel/stdlib", None, name));
    }
    for name in MEMBERS {
        add(standard_symbol("cel/stdlib/member", Some("receiver"), name));
    }
    for (namespace, names) in NAMESPACE_FUNCTIONS {
        for name in *names {
            add(standard_symbol(
                &format!("cel/stdlib/{namespace}"),
                Some("global"),
                name,
            ));
        }
    }
    for library in libraries {
        for function in &library.functions {
            add(library_symbol(&library.id, function));
        }
    }
    symbols
}

pub(in crate::language) fn external_module(symbol: &Symbol) -> String {
    symbol.id.module().to_owned()
}

/// A standard function a call names.
pub(super) struct Standard {
    pub(super) symbol: Symbol,
    /// Whether the call is made on its namespace (`strings.quote(x)`), so the
    /// target is no value to walk.
    pub(super) namespaced: bool,
}

/// The standard function `call` names, if it is one.
pub(super) fn standard_target(call: &CallExpr) -> Option<Standard> {
    let on_namespace = call_namespace(call).filter(|name| namespaces().any(|n| n == *name));
    let (namespace, name, namespaced) = match on_namespace {
        Some(namespace) => (Some(namespace), base_name(&call.func_name), true),
        None => match call.func_name.rsplit_once('.') {
            Some((namespace, name)) if namespaces().any(|n| n == namespace) => {
                (Some(namespace), name, false)
            }
            _ => (None, call.func_name.as_str(), false),
        },
    };
    let (module, owner) = if let Some(namespace) = namespace {
        let known = NAMESPACE_FUNCTIONS
            .iter()
            .any(|(candidate, names)| *candidate == namespace && names.contains(&name));
        if !known {
            return None;
        }
        (format!("cel/stdlib/{namespace}"), Some("global"))
    } else if call.target.is_none() && GLOBALS.contains(&name) {
        ("cel/stdlib".to_owned(), None)
    } else if call.target.is_some() && MEMBERS.contains(&name) {
        ("cel/stdlib/member".to_owned(), Some("receiver"))
    } else {
        return None;
    };
    Some(Standard {
        symbol: standard_symbol(&module, owner, name),
        namespaced,
    })
}

fn standard_symbol(module: &str, owner: Option<&str>, name: &str) -> Symbol {
    let owner = owner.into_iter().collect::<Vec<_>>();
    let id = SymbolId::new(module, &owner, name, SymbolKind::Function);
    external_symbol(id, SymbolKind::Function, name, None)
}

/// A library function a call names.
pub(super) struct LibraryCall {
    pub(super) symbol: Symbol,
    /// Whether the call is made on its namespace (`regex.matches(a, b)`).
    pub(super) namespaced: bool,
}

/// The function of `libraries` that `call` names: a global one by its name or
/// qualified by the namespace the call is made on, or a member of a value.
pub(super) fn library_target(
    call: &CallExpr,
    libraries: &[CelLibraryManifest],
) -> Option<LibraryCall> {
    let namespace = call_namespace(call);
    let qualified =
        namespace.map(|namespace| format!("{namespace}.{}", base_name(&call.func_name)));
    for library in libraries {
        for function in &library.functions {
            let global = function.receiver.is_none()
                && (function.name == call.func_name
                    || qualified.as_deref() == Some(function.name.as_str()));
            let member = function.receiver.is_some()
                && function.name == call.func_name
                && call.target.is_some();
            if global || member {
                return Some(LibraryCall {
                    symbol: library_symbol(&library.id, function),
                    namespaced: global && qualified.as_deref() == Some(function.name.as_str()),
                });
            }
        }
    }
    None
}

fn library_symbol(library: &str, function: &Function) -> Symbol {
    let receiver = function.receiver.as_deref().unwrap_or("global");
    let id = SymbolId::new(
        &format!("cel/library/{library}"),
        &[receiver],
        function_name(function),
        SymbolKind::Function,
    );
    external_symbol(
        id,
        SymbolKind::Function,
        function_name(function),
        Some(&function.doc),
    )
}

fn function_name(function: &Function) -> &str {
    base_name(&function.name)
}

/// A symbol of the plugins' functions and of the environment, which no file
/// of the project declares.
pub(super) fn external_symbol(
    id: SymbolId,
    kind: SymbolKind,
    name: &str,
    doc: Option<&str>,
) -> Symbol {
    Symbol {
        doc: doc.map(str::to_owned),
        ..Symbol::new(
            id,
            kind,
            Visibility::Public,
            PathBuf::from("@cel/library"),
            Span {
                start: Position { line: 1, col: 1 },
                end: Position { line: 1, col: 1 },
            },
            name,
        )
    }
}
