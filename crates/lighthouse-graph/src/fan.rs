//! Fan-out inside a query: [`Db::par_map`].
//!
//! # Why the fan-out drains itself and waits do not help
//!
//! A query that fans out holds a claim on its node while it waits for its
//! parts, and a part may wait for another claimed node. Two ways to wait
//! starve a pool or deadlock:
//!
//! - **Parking every worker on a claim** starves the pool when the owner of
//!   the claim needs a worker to finish. Here the owner never does: the thread
//!   that runs `par_map` runs its items itself, helpers only take some of
//!   them. A claim's owner therefore always makes progress on its own thread,
//!   and a wait ends when the owner's chain does.
//! - **Helping with whatever work is pending while waiting** (`rayon`'s
//!   `yield_now`, or its `join` and `scope` waits, which steal) stacks the
//!   helped job on top of the waiting frame. If that job needs a node claimed
//!   by a frame lower on the same stack, nothing can finish: the lower frame
//!   cannot resume until the job returns. This is a deadlock that is no cycle
//!   among queries, so cycle detection cannot report it, and it needs no more
//!   than two threads.
//!
//! So a worker that waits on a claim parks (and cannot starve anything), and a
//! thread that waits for its own items parks too, instead of stealing. Queries
//! fan out with `par_map`, never with `par_iter`, `join` or `scope`, which
//! steal while they wait.

use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread::Result as Joined,
};

use crate::{Db, node::Locked};

struct Fan<I, R> {
    items: Vec<I>,
    next: AtomicUsize,
    results: Mutex<Vec<Option<Joined<R>>>>,
    left: Mutex<usize>,
    done: Condvar,
    run: Run<I, R>,
}

type Run<I, R> = Box<dyn Fn(&Db, &I) -> R + Send + Sync>;

impl<I, R> Fan<I, R> {
    /// Runs items until none is left to take.
    fn drain(&self, db: &Db) {
        loop {
            let at = self.next.fetch_add(1, Ordering::AcqRel);
            let Some(item) = self.items.get(at) else {
                return;
            };
            let result = catch_unwind(AssertUnwindSafe(|| (self.run)(db, item)));
            self.results.locked()[at] = Some(result);
            let mut left = self.left.locked();
            *left -= 1;
            if *left == 0 {
                self.done.notify_all();
            }
        }
    }

    fn wait(&self) {
        let mut left = self.left.locked();
        while *left > 0 {
            left = self
                .done
                .wait(left)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// Runs `run` on every item, on the calling thread and on the threads of
/// the current `rayon` pool that are free, and returns the results in
/// the order of the items. What `run` reads through the handle it is
/// given is read by the query that called `par_map`.
///
/// A claimed node that an item waits for is run by its owner, and a cycle
/// that runs through items is an [`Error::Cycle`](crate::Error::Cycle)
/// like any other. See the module docs for why this is the way to fan
/// out. A panic in `run` is raised again here.
pub fn par_map<I, R>(
    db: &Db,
    items: Vec<I>,
    run: impl Fn(&Db, &I) -> R + Send + Sync + 'static,
) -> Vec<R>
where
    I: Send + Sync + 'static,
    R: Send + 'static,
{
    let count = items.len();
    let fan = Arc::new(Fan {
        results: Mutex::new((0..count).map(|_| None).collect()),
        left: Mutex::new(count),
        next: AtomicUsize::new(0),
        items,
        done: Condvar::new(),
        run: Box::new(run),
    });
    let helpers = count.saturating_sub(1).min(rayon::current_num_threads());
    for _ in 0..helpers {
        let (fan, db) = (
            fan.clone(),
            Db {
                shared: db.shared.clone(),
                frame: db.frame.clone(),
            },
        );
        rayon::spawn(move || fan.drain(&db));
    }
    fan.drain(db);
    fan.wait();
    let results = std::mem::take(&mut *fan.results.locked());
    results
        .into_iter()
        .map(|result| match result.expect("every item ran") {
            Ok(value) => value,
            Err(panic) => resume_unwind(panic),
        })
        .collect()
}
