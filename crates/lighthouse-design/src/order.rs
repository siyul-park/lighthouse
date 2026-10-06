use lighthouse_model::{Diagnostic, Symbol, SymbolKind};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{
    finding,
    layout::{declarations, exposed, has_word_prefix, owner_key},
    skipped,
};

const ID: &str = "design/declaration-groups";

#[derive(Deserialize)]
struct Options {
    groups: Vec<String>,
    constructor_prefixes: Vec<String>,
    hook_names: Vec<String>,
    constructors_first: bool,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

fn check(meta: &RuleMeta, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for module in declarations(ctx) {
        let grouped: Vec<(&Symbol, usize)> = module
            .iter()
            .filter_map(|s| Some((*s, group_of(s, &options)?)))
            .collect();
        found.extend(out_of_place(meta, &grouped, &options));
        if options.constructors_first {
            found.extend(late_constructors(meta, &module, &options));
        }
    }
    Ok(found)
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

/// Declarations whose group breaks the longest in-order run: the fewest
/// declarations that, moved, would put the file in order.
fn out_of_place(
    meta: &lighthouse_plugin::RuleMeta,
    grouped: &[(&Symbol, usize)],
    options: &Options,
) -> Vec<Diagnostic> {
    let keep = longest_in_order(&grouped.iter().map(|(_, g)| *g).collect::<Vec<_>>());
    let mut found = Vec::new();
    for (at, (symbol, group)) in grouped.iter().enumerate() {
        if keep[at] {
            continue;
        }
        let after = grouped[..at]
            .iter()
            .enumerate()
            .rev()
            .find(|(i, (_, g))| keep[*i] && g <= group)
            .map(|(_, (s, _))| s.name.as_str());
        let name = &options.groups[*group];
        found.push(finding(
            meta,
            symbol,
            format!(
                "{} {} belongs to the `{name}` group and must {}",
                symbol.kind.as_str(),
                symbol.name,
                after.map_or_else(
                    || "come before the declarations above it".to_owned(),
                    |a| format!("follow {a}")
                )
            ),
            json!({
                "declaration": symbol.id.as_str(),
                "group": name,
                "expected_after": after,
            }),
        ));
    }
    found
}

/// Marks a longest non-decreasing subsequence of `groups`; the first of equally
/// long ones, so the verdict does not change between runs.
fn longest_in_order(groups: &[usize]) -> Vec<bool> {
    let n = groups.len();
    let mut best = vec![1usize; n];
    let mut from = vec![usize::MAX; n];
    for i in 0..n {
        for j in 0..i {
            if groups[j] <= groups[i] && best[j] + 1 > best[i] {
                best[i] = best[j] + 1;
                from[i] = j;
            }
        }
    }
    let mut keep = vec![false; n];
    let Some(mut at) = (0..n).max_by_key(|&i| (best[i], std::cmp::Reverse(i))) else {
        return keep;
    };
    loop {
        keep[at] = true;
        match from[at] {
            usize::MAX => break,
            previous => at = previous,
        }
    }
    keep
}

/// A constructor-named method that follows another method of its owner.
fn late_constructors(meta: &RuleMeta, module: &[&Symbol], options: &Options) -> Vec<Diagnostic> {
    let methods: Vec<(&Symbol, String)> = module
        .iter()
        .filter(|s| s.kind == SymbolKind::Method)
        .filter_map(|s| Some((*s, owner_key(s)?)))
        .collect();
    let mut found = Vec::new();
    for (at, (symbol, owner)) in methods.iter().enumerate() {
        let constructor =
            exposed(symbol) && is_constructor(&symbol.name, &options.constructor_prefixes);
        let earlier = methods[..at].iter().find(|(other, key)| {
            key == owner
                && !(exposed(other) && is_constructor(&other.name, &options.constructor_prefixes))
        });
        let (true, Some((first, _))) = (constructor, earlier) else {
            continue;
        };
        found.push(finding(
            meta,
            symbol,
            format!(
                "constructor {} must precede the other methods of its type, such as {}",
                symbol.name, first.name
            ),
            json!({
                "declaration": symbol.id.as_str(),
                "group": "constructor",
                "expected_after": Option::<&str>::None,
            }),
        ));
    }
    found
}
