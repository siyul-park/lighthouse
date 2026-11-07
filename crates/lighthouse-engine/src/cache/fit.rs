//! The budget of the cache directory: the databases and what plugins keep
//! together stay under one limit, whatever writes there. A run under the limit
//! pays one pass over the directory's sizes; one over it gives back, in this
//! order, the free pages of the databases, what plugins left that no run reads,
//! the states and values of the graph (retained states first, then the values
//! of lower durability first, the least recently used first), and last the
//! answers of models, which cost money to get again. What goes is derived:
//! a result is the same with or without it.

use std::{fs, path::Path};

use lighthouse_graph::Graph;
use rusqlite::ErrorCode;

use crate::engine::CacheSettings;

use super::{Cache, Error, plugins::forget_leftovers};

/// A trim goes down to this part of the limit, so that a run that is just over
/// does not trim every time.
const TARGET: f64 = 0.9;

/// Passes over the sizes after a trim that did not free as many bytes as it
/// meant to: pages of a file hold more than the values asked for.
const ROUNDS: usize = 4;

/// Brings the cache directory of `settings` under its limit, if it is over;
/// what could not be done is said in the returned notice. `graph` is the
/// graph kept in it, if one is open. Eviction is best effort: a database
/// another run holds is left for the next run, and nothing a concurrent run
/// reads is ever removed from under it (a database is trimmed in its own
/// transaction; a kept answer that is gone is asked of the plugin again).
pub(crate) fn fit(settings: Option<&CacheSettings>, graph: Option<&mut Graph>) -> Option<String> {
    let settings = settings?;
    squeeze(&settings.dir, settings.limit, graph)
        .err()
        .map(|e| format!("the cache was not trimmed: {e}"))
}

fn squeeze(dir: &Path, limit: u64, graph: Option<&mut Graph>) -> Result<(), Error> {
    if size(dir) <= limit {
        return Ok(());
    }
    let target = (limit as f64 * TARGET) as u64;
    let over = || size(dir).saturating_sub(target);
    let mut graph = graph;
    if let Some(graph) = graph.as_deref_mut() {
        unless_busy(lighthouse_graph::trim(graph, 0).map_err(Error::from))?;
    }
    let cache = Cache::open(dir).ok();
    if let Some(cache) = &cache {
        unless_busy(cache.trim(0))?;
    }
    if over() > 0 {
        forget_leftovers(dir);
    }
    if let Some(graph) = graph {
        until_fit(over(), over, |bytes| {
            unless_busy(lighthouse_graph::trim(graph, bytes).map_err(Error::from))
        })?;
    }
    if let Some(cache) = &cache {
        until_fit(over(), over, |bytes| unless_busy(cache.trim(bytes)))?;
    }
    Ok(())
}

/// The bytes of every file under `dir`, the logs of the databases included.
fn size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let meta = fs::symlink_metadata(entry.path()).ok()?;
            Some(if meta.is_dir() {
                size(&entry.path())
            } else {
                meta.len()
            })
        })
        .sum()
}

/// Trims by `over` bytes, and again by what is still over, while a trim frees
/// anything.
fn until_fit(
    mut over: u64,
    measure: impl Fn() -> u64,
    mut trim: impl FnMut(u64) -> Result<(), Error>,
) -> Result<(), Error> {
    for _ in 0..ROUNDS {
        if over == 0 {
            break;
        }
        trim(over)?;
        let now = measure();
        if now >= over {
            break;
        }
        over = now;
    }
    Ok(())
}

/// A database that another run holds is not a failure: the trim is left for
/// the next run.
fn unless_busy(result: Result<(), Error>) -> Result<(), Error> {
    match result {
        Err(Error::Sqlite(error) | Error::Graph(lighthouse_graph::Error::Store(error)))
            if matches!(
                error,
                rusqlite::Error::SqliteFailure(ref failure, _)
                    if matches!(failure.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
            ) =>
        {
            Ok(())
        }
        other => other,
    }
}
