use lighthouse_model::{Diagnostic, SymbolKind, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, skipped};

const ID: &str = "design/exported-doc";

#[derive(Deserialize)]
struct Options {
    kinds: Vec<String>,
    include_internal: bool,
    exempt_methods: Vec<String>,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

fn check(meta: &RuleMeta, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, _)) = ctx.file.filter(|_| !skipped(ctx)) else {
        return Ok(Vec::new());
    };
    let exposed = |visibility: Visibility| {
        visibility == Visibility::Public
            || (options.include_internal && visibility == Visibility::Internal)
    };
    let mut found = Vec::new();
    for symbol in ctx.project.symbols_in(&file.path) {
        let owner = symbol.owner.as_ref().and_then(|o| ctx.project.symbol(o));
        let named = options.kinds.iter().any(|k| k == symbol.kind.as_str());
        let reachable = exposed(symbol.visibility)
            && owner.is_none_or(|o| exposed(o.visibility) && o.kind != SymbolKind::Interface);
        let exempt =
            symbol.kind == SymbolKind::Method && options.exempt_methods.contains(&symbol.name);
        if !named || !reachable || exempt || symbol.doc.is_some() {
            continue;
        }
        found.push(finding(
            meta,
            symbol,
            format!(
                "exported {} {} must have a doc comment",
                symbol.kind.as_str(),
                symbol.name
            ),
            json!({ "symbol": symbol.id.as_str() }),
        ));
    }
    Ok(found)
}
