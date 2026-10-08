use lighthouse_model::{Diagnostic, Module, Symbol, SymbolKind};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, skipped};

const ID: &str = "design/no-redundant-qualifiers";

#[derive(Deserialize)]
struct Options {
    kinds: Vec<String>,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(DecisionRule::new(ID, &[], check))
}

/// A top-level exported name that carries the name of its module as a prefix
/// or suffix repeats
/// what every caller already writes: `provider.ProviderConfig`.
fn check(meta: &RuleManifest, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let Some((file, _)) = ctx.file else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for symbol in ctx.project.symbols_in(&file.path) {
        let candidate = symbol.owner.is_none()
            && symbol.visibility != lighthouse_model::Visibility::Private
            && options.kinds.iter().any(|k| k == symbol.kind.as_str())
            && !ctx.project.in_test(&symbol.id);
        let Some(module) = ctx.project.module(symbol.id.module()).filter(|_| candidate) else {
            continue;
        };
        let Some(repeated) = repeated(symbol, module) else {
            continue;
        };
        found.push(finding(
            meta,
            symbol,
            format!(
                "{} {} repeats its module name `{repeated}`; callers already write it",
                symbol.kind.as_str(),
                symbol.name
            ),
            json!({ "name": symbol.name, "repeated": repeated }),
        ));
    }
    Ok(found)
}

/// The module name a longer symbol name starts or ends with, whole words
/// only. A name equal to the module name is the module's primary concept and
/// is never a repetition.
fn repeated(symbol: &Symbol, module: &Module) -> Option<String> {
    if symbol.kind == SymbolKind::Function && symbol.name == "main" {
        return None;
    }
    let name = words(&symbol.name);
    let last = module.path.rsplit('/').next().unwrap_or(&module.path);
    let spelled = module.name.as_deref().unwrap_or(last);
    for candidate in [spelled, last] {
        let prefix = words(candidate);
        let longer = name.len() > prefix.len();
        let qualified =
            !prefix.is_empty() && longer && (name.starts_with(&prefix) || name.ends_with(&prefix));
        if qualified {
            return Some(candidate.to_owned());
        }
    }
    None
}

/// Lower-case words of an identifier: split at `_`, at a lower-to-upper
/// change, and before the last capital of an acronym followed by a word
/// (`SDKThing` is `sdk`, `thing`).
fn words(identifier: &str) -> Vec<String> {
    let chars: Vec<char> = identifier.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            flush(&mut words, &mut current);
            continue;
        }
        let after_lower = i > 0 && chars[i - 1].is_lowercase() && c.is_uppercase();
        let acronym_end = i > 0
            && chars[i - 1].is_uppercase()
            && c.is_uppercase()
            && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if after_lower || acronym_end {
            flush(&mut words, &mut current);
        }
        current.extend(c.to_lowercase());
    }
    flush(&mut words, &mut current);
    words
}

fn flush(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}
