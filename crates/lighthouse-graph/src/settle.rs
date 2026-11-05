//! Validating or running a node whose claim is held.

use std::sync::{Arc, atomic::Ordering};

use crate::{
    Db, Durability, Error,
    frame::Frame,
    node::{Cause, Dep, Hash, Locked, Node, NodeId, Slot},
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

impl Db {
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
        match self.revalidate(&snapshot)? {
            Some(moved) => self.execute(slot, kind, moved),
            None => self.green(slot, kind, want_value),
        }
    }

    /// `None` when nothing the node read has moved since it was verified:
    /// no input of its durability changed, or each dependency, validated in
    /// order, has not changed after it. Otherwise why it must run.
    fn revalidate(&self, snapshot: &Snapshot) -> Result<Option<Cause>, Error> {
        let revision = self.shared.revision.load(Ordering::Acquire);
        let quiet = self.shared.changed[snapshot.durability.index()].load(Ordering::Acquire)
            <= snapshot.verified_at;
        if snapshot.verified_at == revision || quiet {
            return Ok(None);
        }
        for dep in snapshot.deps.iter() {
            let known = self.shared.registered(&dep.id.kind);
            let pulled = known
                .map(|kind| self.pull(&dep.id, kind, false))
                .transpose()?;
            let moved = match pulled {
                // An input that was unset when read and still is.
                Some(pulled) if pulled.missing && dep.hash == [0; 32] => None,
                Some(pulled) if !pulled.missing => (pulled.changed_at > snapshot.verified_at)
                    .then(|| Cause::DepMoved(dep.id.clone())),
                _ => Some(Cause::UnknownDep(dep.id.clone())),
            };
            if moved.is_some() {
                return Ok(moved);
            }
        }
        Ok(None)
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
                self.shared.dirty.mark(slot, &mut node);
            }
            self.shared.dirty.touch(slot, &mut node);
            (want_value && node.value.is_none(), node.hash)
        };
        if self.shared.sampled() {
            self.verify(&slot.id, kind, expected)?;
        }
        if needs_value && !self.load_value(slot, kind, expected)? {
            return self.execute(slot, kind, Cause::Evicted);
        }
        Ok(())
    }

    /// Verify mode: runs the node again and compares.
    fn verify(&self, id: &NodeId, kind: &'static dyn Kind, expected: Hash) -> Result<(), Error> {
        let inside = self.within(&Frame::new(id.clone(), self.frame.clone()));
        let output = kind.execute(&inside, &id.key)?;
        match hash(&output.blob) == expected {
            true => Ok(()),
            false => Err(Error::Impure(self.shared.label(id))),
        }
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
        let output = kind.execute(self, &slot.id.key)?;
        let new_hash = hash(&output.blob);
        let (deps, durability) = frame.take_reads();
        let revision = self.shared.revision.load(Ordering::Acquire);
        self.shared.executed.locked().push(slot.id.clone());
        let mut node = slot.node.locked();
        if !(node.present && node.hash == new_hash) {
            node.changed_at = revision;
        }
        node.present = true;
        node.hash = new_hash;
        node.deps = deps.into();
        node.durability = durability;
        node.verified_at = revision;
        node.value = Some(output.value);
        node.blob = Some(output.blob);
        node.value_dirty = true;
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
