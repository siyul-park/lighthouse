//! Validating or running a node whose claim is held.

use std::{
    any::Any,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

use crate::{
    Durability, Error, Graph,
    frame::Frame,
    node::{Cause, Dep, Hash, Locked, Node, NodeId, Slot, Trace},
    query::{Kind, hash},
    shared::now,
};

/// What validation needs of a node, read once.
struct Snapshot {
    present: bool,
    verified_at: u64,
    durability: Durability,
    deps: Arc<[Dep]>,
}

impl Graph {
    /// Runs with the claim held, `self` being the handle of the frame.
    pub(crate) fn settle(
        &self,
        slot: &Arc<Slot>,
        kind: &'static dyn Kind,
        want_value: bool,
    ) -> Result<(), Error> {
        let snapshot = Snapshot::of(&slot.node.locked());
        if !snapshot.present {
            return self.execute(slot, kind, Cause::Computed);
        }
        match self.revalidate(&snapshot, kind)? {
            Some(_) if self.take_back(slot, kind)? => Ok(()),
            Some(moved) => self.execute(slot, kind, moved),
            None => self.green(slot, kind, want_value),
        }
    }

    /// `None` when nothing the node read has moved since it was verified:
    /// no input of its durability changed, or each dependency, validated in
    /// order, is what it was when the node read it. Otherwise why it must run.
    fn revalidate(&self, snapshot: &Snapshot, kind: &dyn Kind) -> Result<Option<Cause>, Error> {
        let revision = self.shared.revision.load(Ordering::Acquire);
        let quiet = self.shared.changed[snapshot.durability.index()].load(Ordering::Acquire)
            <= snapshot.verified_at;
        if snapshot.verified_at == revision || quiet {
            return Ok(None);
        }
        self.moved(&snapshot.deps, kind)
    }

    /// The first of the leading dependencies, validated in order, whose hash
    /// is not the one recorded.
    fn moved(&self, deps: &[Dep], kind: &dyn Kind) -> Result<Option<Cause>, Error> {
        let looked = kind.deciding().unwrap_or(usize::MAX);
        for dep in deps.iter().take(looked) {
            let known = self.shared.registered(&dep.id.kind);
            let pulled = known
                .map(|kind| self.pull(&dep.id, kind, false))
                .transpose()?;
            let moved = match pulled {
                // Read as unset: whatever it is set to since is news.
                Some(pulled) if dep.hash == [0; 32] => {
                    (!pulled.missing).then(|| Cause::DepMoved(dep.id.clone()))
                }
                Some(pulled) if !pulled.missing => {
                    (pulled.hash != dep.hash).then(|| Cause::DepMoved(dep.id.clone()))
                }
                _ => Some(Cause::UnknownDep(dep.id.clone())),
            };
            if moved.is_some() {
                return Ok(moved);
            }
        }
        Ok(None)
    }

    /// Takes the newest retained state of the node whose dependencies are all
    /// what that state read, instead of running the query. A dependency that
    /// cannot be pulled any more means the state does not match.
    fn take_back(&self, slot: &Arc<Slot>, kind: &'static dyn Kind) -> Result<bool, Error> {
        let traces = {
            let mut node = slot.node.locked();
            self.shared.load_traces(&mut node, &slot.id)?;
            node.traces.clone()
        };
        for trace in traces {
            match self.moved(&trace.deps, kind) {
                Ok(None) => {
                    let Some(blob) = self.shared.trace_bytes(&trace)? else {
                        continue;
                    };
                    let Some(value) = kind.decode(&blob) else {
                        continue;
                    };
                    let expected = trace.hash;
                    self.take(slot, trace, blob, value)?;
                    self.audit(slot, kind, expected)?;
                    return Ok(true);
                }
                Ok(Some(_)) | Err(Error::Incomplete(_) | Error::Encode(_)) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(false)
    }

    /// Makes a retained state the current one, the current one retained.
    fn take(
        &self,
        slot: &Arc<Slot>,
        trace: Trace,
        blob: Arc<[u8]>,
        value: Arc<dyn Any + Send + Sync>,
    ) -> Result<(), Error> {
        let revision = self.shared.revision.load(Ordering::Acquire);
        let mut node = slot.node.locked();
        let same = node.hash == trace.hash;
        self.shared.retain(&mut node, &slot.id)?;
        node.forget_trace(trace.hash, &trace.deps);
        if !same {
            node.changed_at = revision;
        }
        node.hash = trace.hash;
        node.deps = trace.deps;
        node.durability = trace.durability;
        node.verified_at = revision;
        node.value = Some(value);
        node.blob = Some(blob);
        node.value_dirty = !(same && node.on_disk);
        node.cause = Cause::Retained;
        node.used_at = now();
        self.shared.dirty.mark(slot, &mut node);
        Ok(())
    }

    /// Marks the node verified without running it, and gives it a value if
    /// the pull needs one.
    fn green(
        &self,
        slot: &Arc<Slot>,
        kind: &'static dyn Kind,
        want_value: bool,
    ) -> Result<(), Error> {
        let (needs_value, expected) = {
            let mut node = slot.node.locked();
            let revision = self.shared.revision.load(Ordering::Acquire);
            if node.verified_at != revision {
                node.verified_at = revision;
                node.cause = Cause::Green;
                self.shared.mark_verified(slot, &mut node);
            }
            self.shared.dirty.touch(slot, &mut node);
            (want_value && node.value.is_none(), node.hash)
        };
        self.audit(slot, kind, expected)?;
        if needs_value && !self.load_value(slot, kind, expected)? {
            return self.execute(slot, kind, Cause::Evicted);
        }
        Ok(())
    }

    /// Verify mode, for a sample of the nodes taken without running: runs the
    /// node again and compares, unless that starts a program.
    fn audit(&self, slot: &Slot, kind: &'static dyn Kind, expected: Hash) -> Result<(), Error> {
        if self.shared.sampled() && !kind.starts_program(self, &slot.id.key) {
            self.verify(&slot.id, kind, expected)?;
        }
        Ok(())
    }

    /// Runs the node again and compares.
    fn verify(&self, id: &NodeId, kind: &'static dyn Kind, expected: Hash) -> Result<(), Error> {
        let inside = self.within(&Frame::new(id.clone(), self.frame.clone()));
        let output = kind.execute(&inside, &id.key)?;
        if hash(&output.blob) != expected {
            return Err(Error::Impure(self.shared.label(id)));
        }
        Ok(())
    }

    /// Reads the stored value, if it is there and is what its hash says.
    fn load_value(&self, slot: &Arc<Slot>, kind: &dyn Kind, expected: Hash) -> Result<bool, Error> {
        let bytes = self.shared.store.value(&slot.id.kind, &slot.id.key)?;
        let decoded = bytes
            .as_ref()
            .filter(|bytes| hash(bytes) == expected)
            .and_then(|bytes| kind.decode(bytes));
        let (Some(bytes), Some(value)) = (bytes, decoded) else {
            return Ok(false);
        };
        let mut node = slot.node.locked();
        node.value = Some(value);
        node.blob = Some(bytes.into());
        Ok(true)
    }

    /// Runs the query and keeps the value. An equal hash keeps `changed_at`:
    /// the backdating that lets what read it stay green.
    fn execute(
        &self,
        slot: &Arc<Slot>,
        kind: &'static dyn Kind,
        cause: Cause,
    ) -> Result<(), Error> {
        let Some(frame) = &self.frame else {
            return Ok(());
        };
        let started = Instant::now();
        let output = kind.execute(self, &slot.id.key)?;
        let cost = started.elapsed();
        let new_hash = hash(&output.blob);
        let (deps, durability) = frame.take_reads();
        let revision = self.shared.revision.load(Ordering::Acquire);
        self.shared.executed.locked().push(slot.id.clone());
        let mut node = slot.node.locked();
        let same = node.present && node.hash == new_hash;
        let deps: Arc<[Dep]> = deps.into();
        if node.present && (!same || node.deps != deps) {
            self.shared.retain(&mut node, &slot.id)?;
        }
        if !same {
            node.changed_at = revision;
        }
        node.forget_trace(new_hash, &deps);
        node.present = true;
        node.hash = new_hash;
        node.deps = deps;
        node.durability = durability;
        node.verified_at = revision;
        node.cost = u64::try_from(cost.as_micros()).unwrap_or(u64::MAX);
        node.value = Some(output.value);
        let kept = !kind.transient();
        node.blob = kept.then_some(output.blob);
        // A value the store holds already is the one the hash says.
        node.value_dirty = kept && !(same && node.on_disk);
        node.cause = cause;
        node.used_at = now();
        self.shared.dirty.mark(slot, &mut node);
        Ok(())
    }
}

impl Snapshot {
    fn of(node: &Node) -> Self {
        Self {
            present: node.present,
            verified_at: node.verified_at,
            durability: node.durability,
            deps: node.deps.clone(),
        }
    }
}
