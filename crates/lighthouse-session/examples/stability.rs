//! Reports how far a change travels through a project: the propagation cost
//! and core of its module graph, the same for its symbols, and the functions
//! that are both hard to follow and far reaching.
//!
//! usage: cargo run --release -q --example stability -p lighthouse-session --
//!        --config <lighthouse.toml> <repo>

use std::{
    env,
    path::{Path, PathBuf},
};

use lighthouse_checks::metrics::{
    Measures,
    propagation::{modules, symbols},
};
use lighthouse_model::{SymbolId, SymbolKind};
use lighthouse_session::{CheckRequest, Result, Session, check};
use serde_json::json;

/// How many functions the report names.
const HOTSPOTS: usize = 10;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [flag, config, repo] = args.as_slice() else {
        return Err("usage: stability --config <lighthouse.toml> <repo>".into());
    };
    if flag != "--config" {
        return Err(format!("unexpected argument {flag}").into());
    }
    let config = PathBuf::from(config).canonicalize()?;
    let session = Session::load_in(Path::new(repo), Some(&config))?;
    let checked = check(&session, &CheckRequest::default())?;
    let project = &checked.run.project;

    let (by_module, by_symbol) = (modules(project), symbols(project));
    let core: Vec<&String> = by_module
        .nodes
        .iter()
        .zip(&by_module.stability)
        .filter(|(_, s)| s.core)
        .map(|(name, _)| name)
        .collect();
    let mut hotspots: Vec<_> = by_symbol
        .nodes
        .iter()
        .zip(&by_symbol.stability)
        .filter_map(|(id, reach)| {
            let symbol = project.symbol(&SymbolId::parse(id)?)?;
            let function = matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method);
            let summary = project.function(&symbol.id).filter(|_| function)?;
            let cognitive = Measures::of(project, symbol, summary).cognitive;
            let share = f64::from(reach.vfi) / by_symbol.nodes.len() as f64;
            Some((
                id,
                cognitive,
                reach.vfi,
                round(f64::from(cognitive) * share),
            ))
        })
        .collect();
    hotspots.sort_by(|a, b| b.3.total_cmp(&a.3).then_with(|| a.0.cmp(b.0)));
    hotspots.truncate(HOTSPOTS);

    let report = json!({
        "modules": by_module.nodes.len(),
        "propagationCost": round(by_module.propagation_cost),
        "coreShare": round(by_module.core_share),
        "largestScc": { "size": core.len(), "members": core },
        "cycles": by_module.cycles.len(),
        "symbols": by_symbol.nodes.len(),
        "symbolPropagationCost": round(by_symbol.propagation_cost),
        "hotspots": hotspots
            .iter()
            .map(|(id, cognitive, vfi, score)| json!({
                "id": id, "cognitive": cognitive, "vfi": vfi, "score": score,
            }))
            .collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

/// Four decimals, so that two runs print the same text.
fn round(value: f64) -> f64 {
    (value * 1e4).round() / 1e4
}
