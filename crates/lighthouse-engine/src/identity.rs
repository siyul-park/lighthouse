use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Diagnostic, Fingerprint, Project};

/// Makes the fingerprints of one run unique. Findings that share a rule, file
/// and fingerprint are told apart by a stable discriminator (the finding's
/// symbol, else the symbol that encloses or precedes it) and only findings
/// that still collide get an ordinal. Returns the fingerprints that rest on
/// an ordinal, which moves when an earlier identical finding appears or goes.
pub(crate) fn assign(found: &mut [Diagnostic], project: &Project) -> BTreeSet<Fingerprint> {
    let mut groups: BTreeMap<(String, PathBuf, Fingerprint), Vec<usize>> = BTreeMap::new();
    for (index, d) in found.iter().enumerate() {
        let key = (d.rule_id.clone(), d.file.clone(), d.fingerprint.clone());
        groups.entry(key).or_default().push(index);
    }
    let mut ordinal = BTreeSet::new();
    for members in groups.into_values().filter(|m| m.len() > 1) {
        let mut by_discriminator: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for index in members {
            let discriminator = discriminator(&found[index], project);
            by_discriminator
                .entry(discriminator)
                .or_default()
                .push(index);
        }
        for (discriminator, same) in by_discriminator {
            for (n, &index) in same.iter().enumerate() {
                let base = &found[index].fingerprint;
                let base = if discriminator.is_empty() {
                    base.clone()
                } else {
                    base.discriminate(&discriminator)
                };
                found[index].fingerprint = base.occurrence(n);
                if same.len() > 1 {
                    ordinal.insert(found[index].fingerprint.clone());
                }
            }
        }
    }
    ordinal
}

fn discriminator(d: &Diagnostic, project: &Project) -> String {
    if let Some(symbol) = &d.symbol {
        return symbol.clone();
    }
    let symbols: Vec<_> = project.symbols_in(&d.file).collect();
    let at = d.span.start;
    let innermost = symbols
        .iter()
        .filter(|s| s.span.start <= at && at <= s.span.end)
        .max_by_key(|s| s.span.start);
    let preceding = symbols
        .iter()
        .filter(|s| s.span.end < at)
        .max_by_key(|s| s.span.end);
    innermost
        .or(preceding)
        .map_or_else(String::new, |s| s.id.as_str().to_owned())
}
