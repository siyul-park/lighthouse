//! The keys `reorder` fixes of the `design` pack sort by.

use std::sync::LazyLock;

use lighthouse_model::{Symbol, SymbolKind, SymbolRole};
use lighthouse_plugin::{Error, KeyCtx, OrderKey, OrderKeyManifest};
use lighthouse_spec::Catalog;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::layout::{exposed, has_word_prefix, is_declaration, owner_key};

const ID: &str = "design/declaration-groups";

#[derive(Deserialize)]
struct Options {
    groups: Vec<String>,
    constructor_prefixes: Vec<String>,
    hook_names: Vec<String>,
}

/// The order keys of the pack.
pub(crate) fn keys() -> Vec<Box<dyn OrderKey>> {
    vec![Box::new(GroupKey), Box::new(ConstructorKey)]
}

/// Orders declarations the way the `declaration-groups` rule does: by the
/// index of their group in the rule's order. A declaration the order does not
/// mention, test code and generated code have no rank and stay where they are.
pub(crate) struct GroupKey;

impl OrderKey for GroupKey {
    fn manifest(&self) -> &OrderKeyManifest {
        static MANIFEST: LazyLock<OrderKeyManifest> = LazyLock::new(|| {
            OrderKeyManifest {
            id: "design/group".to_owned(),
            description: "the declaration group of `design/declaration-groups`, from public contract to private mechanics".to_owned(),
        }
        });
        &MANIFEST
    }

    fn rank(&self, ctx: &KeyCtx, symbol: &Symbol) -> Result<Option<u64>, Error> {
        let project = ctx.project;
        let Some(file) = project.file(&symbol.file) else {
            return Ok(None);
        };
        let judged = if file.test {
            symbol.role == Some(SymbolRole::Fixture)
        } else {
            !project.in_test(&symbol.id)
        };
        if file.generated || !judged || !is_declaration(project, symbol) {
            return Ok(None);
        }
        let configured = if ctx.rule == ID {
            ctx.options.clone()
        } else {
            Map::new()
        };
        let group = group_index(Some(ctx.language), &configured)?;
        Ok(group(symbol).map(|g| g as u64))
    }
}

/// Orders the methods of a type constructors first: a constructor (a public
/// method named like one) ranks before the other methods. It orders nothing
/// else, and nothing at all unless `constructors_first` is on.
pub(crate) struct ConstructorKey;

impl OrderKey for ConstructorKey {
    fn manifest(&self) -> &OrderKeyManifest {
        static MANIFEST: LazyLock<OrderKeyManifest> = LazyLock::new(|| OrderKeyManifest {
            id: "design/constructor-first".to_owned(),
            description: "constructors before the other methods of their type".to_owned(),
        });
        &MANIFEST
    }

    fn rank(&self, ctx: &KeyCtx, symbol: &Symbol) -> Result<Option<u64>, Error> {
        let generated = ctx.project.file(&symbol.file).is_none_or(|f| f.generated);
        if symbol.kind != SymbolKind::Method
            || owner_key(symbol).is_none()
            || generated
            || ctx.project.in_test(&symbol.id)
        {
            return Ok(None);
        }
        let configured = if ctx.rule == ID {
            ctx.options.clone()
        } else {
            Map::new()
        };
        let options = resolved(Some(ctx.language), &configured)?;
        let constructor =
            exposed(symbol) && is_constructor(&symbol.name, &options.constructor_prefixes);
        Ok(Some(u64::from(!constructor)))
    }
}

/// The group index of a symbol under the default order of `language`, for the
/// rules whose verdict depends on where the order places a declaration.
pub(crate) fn group_index(
    language: Option<&str>,
    configured: &Map<String, Value>,
) -> Result<impl Fn(&Symbol) -> Option<usize> + use<>, Error> {
    let options = resolved(language, configured)?;
    Ok(move |symbol: &Symbol| group_of(symbol, &options))
}

/// The options of `design/declaration-groups` for `language` over `configured`.
fn resolved(language: Option<&str>, configured: &Map<String, Value>) -> Result<Options, Error> {
    let fail = |message: String| Error::Options {
        rule: ID.to_owned(),
        message,
    };
    let decision = Catalog::bundled()
        .decision(ID)
        .ok_or_else(|| fail("decision missing from the bundled catalog".to_owned()))?;
    let resolved = decision.rule_options(configured, language)?;
    serde_json::from_value(Value::Object(resolved)).map_err(|e| fail(e.to_string()))
}

/// Index into the configured group list of the first group that fits the
/// symbol, or `None` when the order does not mention the symbol's group. The
/// candidates run from the most specific group to the least.
fn group_of(symbol: &Symbol, options: &Options) -> Option<usize> {
    let exposed = exposed(symbol);
    let visible =
        |public: &'static str, private: &'static str| if exposed { public } else { private };
    let mut candidates: Vec<&str> = Vec::new();
    match symbol.kind {
        SymbolKind::Type | SymbolKind::Interface => {
            candidates.extend([visible("public-type", "private-type"), "type"]);
        }
        SymbolKind::Const if symbol.owner.is_some() => candidates.push("type"),
        SymbolKind::Const => candidates.extend([visible("public-const", "private-const"), "const"]),
        SymbolKind::Var => candidates.push("var"),
        SymbolKind::Function => {
            if symbol.name == "init" {
                candidates.push("init");
            }
            if exposed && is_constructor(&symbol.name, &options.constructor_prefixes) {
                candidates.push("constructor");
            }
            candidates.extend([visible("public-function", "private-function")]);
        }
        SymbolKind::Method => {
            candidates.push("type");
            if exposed && options.hook_names.contains(&symbol.name) {
                candidates.push("hook");
            }
            candidates.push(visible("public-method", "private-function"));
        }
        _ => return None,
    }
    candidates
        .into_iter()
        .find_map(|c| options.groups.iter().position(|g| g == c))
}

fn is_constructor(name: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|p| has_word_prefix(name, p))
}
