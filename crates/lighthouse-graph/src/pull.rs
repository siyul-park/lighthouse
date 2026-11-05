//! Pulling a node: red-green validation, backdating, and the claim that makes
//! a second puller wait.

use std::{
    any::Any,
    sync::{Arc, MutexGuard, PoisonError, atomic::Ordering},
};

use crate::{
    Durability, Error, Graph,
    frame::{Frame, cycle},
    node::{Hash, Locked, Node, NodeId, Slot, State},
    query::Kind,
};

/// What a pull tells its caller about the node.
pub(crate) struct Pulled {
    pub hash: Hash,
    pub changed_at: u64,
    pub durability: Durability,
    pub value: Option<Arc<dyn Any + Send + Sync>>,
    /// The node has never been computed or set.
    pub missing: bool,
}

/// Releases the claim on a node when dropped, even by a panic in a query.
struct Claim<'a>(&'a Slot);

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.0.node.locked().state = State::Idle;
        self.0.ready.notify_all();
    }
}

impl Graph {
    /// Brings the node to the current revision and returns what it is. With
    /// `want_value` the value is loaded or computed too; a validation does
    /// not need it.
    pub(crate) fn pull(
        &self,
        id: &NodeId,
        kind: &'static dyn Kind,
        want_value: bool,
    ) -> Result<Pulled, Error> {
        let slot = self.shared.slot(id);
        loop {
            let mut node = slot.node.locked();
            self.shared.load(&mut node, id)?;
            if let Some(pulled) = self.current(&slot, &mut node, kind, want_value)? {
                return Ok(pulled);
            }
            if let State::Running(owner) = &node.state {
                let owner = owner.clone();
                self.block(&slot, node, &owner)?;
                continue;
            }
            let frame = Frame::new(id.clone(), self.frame.clone());
            node.state = State::Running(frame.clone());
            drop(node);
            self.run_claimed(&slot, &frame, kind, want_value)?;
        }
    }

    fn current(
        &self,
        slot: &Arc<Slot>,
        node: &mut Node,
        kind: &dyn Kind,
        want_value: bool,
    ) -> Result<Option<Pulled>, Error> {
        if kind.is_input() {
            return Ok(Some(Pulled::input(node, want_value)));
        }
        let revision = self.shared.revision.load(Ordering::Acquire);
        if !node.present || node.verified_at != revision || (want_value && node.value.is_none()) {
            return Ok(None);
        }
        self.shared.dirty.touch(slot, node);
        Ok(Some(Pulled::of(node, want_value)))
    }

    fn run_claimed(
        &self,
        slot: &Arc<Slot>,
        frame: &Arc<Frame>,
        kind: &'static dyn Kind,
        want_value: bool,
    ) -> Result<(), Error> {
        let _claim = Claim(slot);
        if let Some(caller) = &self.frame {
            caller.add_child(frame);
        }
        let result = self.within(frame).settle(slot, kind, want_value);
        if let Some(caller) = &self.frame {
            caller.remove_child(frame);
        }
        result
    }

    /// The handle of a query running in `frame`.
    pub(crate) fn within(&self, frame: &Arc<Frame>) -> Graph {
        Graph {
            shared: self.shared.clone(),
            frame: Some(frame.clone()),
        }
    }

    /// Waits for the claim on a node to be released, unless that would close
    /// a cycle. The wait parks the thread: see [`Graph::par_map`] for why that
    /// cannot starve the pool, and why helping with other work could.
    fn block(
        &self,
        slot: &Slot,
        node: MutexGuard<'_, Node>,
        owner: &Arc<Frame>,
    ) -> Result<(), Error> {
        if let Some(me) = &self.frame {
            let _waits = self.shared.waits.locked();
            if let Some(path) = cycle(me, owner) {
                return Err(Error::Cycle(
                    path.iter().map(|node| self.shared.label(node)).collect(),
                ));
            }
            me.add_waiting(owner);
        }
        drop(
            slot.ready
                .wait(node)
                .unwrap_or_else(PoisonError::into_inner),
        );
        if let Some(me) = &self.frame {
            me.remove_waiting(owner);
        }
        Ok(())
    }
}

impl Pulled {
    /// An input as this process has it: one that was not set here has no
    /// value, whatever the store remembers of it, and reads as the zero hash.
    fn input(node: &Node, want_value: bool) -> Self {
        let mut pulled = Self::of(node, want_value);
        if node.value.is_none() {
            pulled.missing = true;
            pulled.hash = [0; 32];
        }
        pulled
    }

    fn of(node: &Node, want_value: bool) -> Self {
        Self {
            hash: node.hash,
            changed_at: node.changed_at,
            durability: node.durability,
            value: if want_value { node.value.clone() } else { None },
            missing: !node.present,
        }
    }
}
