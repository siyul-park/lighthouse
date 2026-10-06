use lighthouse_model::{Diagnostic, Symbol, SymbolKind};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{
    finding,
    layout::{declarations, exposed, owner_key},
    skipped,
};

const ID: &str = "design/related-symbols-close";

#[derive(Deserialize)]
struct Options {
    visibility_groups: bool,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

/// The members of one owner are contiguous: nothing but members of that owner
/// may separate two of them.
fn check(meta: &RuleMeta, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for module in declarations(ctx) {
        let members: Vec<(usize, String)> = module
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == SymbolKind::Method)
            .filter_map(|(at, s)| Some((at, owner_key(s)?)))
            .collect();
        for (n, (at, owner)) in members.iter().enumerate() {
            let previous = members[..n].iter().rev().find(|(_, key)| key == owner);
            let Some((before, _)) = previous else {
                continue;
            };
            let separators: Vec<&Symbol> = module[before + 1..*at]
                .iter()
                .copied()
                .filter(|s| separates(s, module[*at], owner, &options))
                .collect();
            let Some(first) = separators.first() else {
                continue;
            };
            let symbol = module[*at];
            found.push(finding(
                meta,
                symbol,
                format!(
                    "method {} is separated from the other methods of its type by {} ({}); keep an owner's methods together",
                    symbol.name,
                    plural(separators.len()),
                    first.name
                ),
                json!({
                    "symbols": [module[*before].id.as_str(), first.id.as_str(), symbol.id.as_str()],
                    "distance": separators.len(),
                }),
            ));
        }
    }
    Ok(found)
}

fn plural(n: usize) -> String {
    format!("{n} other declaration{}", if n == 1 { "" } else { "s" })
}

/// Whether `between` splits `owner`'s members around `member`. Anything that
/// is not a method does; a method of another owner does too, unless layouts
/// group methods by visibility and it sits in the other visibility group.
fn separates(between: &Symbol, member: &Symbol, owner: &str, options: &Options) -> bool {
    if between.kind != SymbolKind::Method {
        return true;
    }
    if owner_key(between).as_deref() == Some(owner) {
        return false;
    }
    !options.visibility_groups || exposed(between) == exposed(member)
}
