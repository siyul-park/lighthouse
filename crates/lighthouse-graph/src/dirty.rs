use std::sync::{Arc, Mutex};

use crate::{
    node::{Locked, Node, Slot},
    shared::now,
};

/// A touch is written to the store only when the stored one is this old, in
/// seconds, so that a run that only reads does not write every node.
const TOUCH_AFTER: u64 = 24 * 60 * 60;

/// The nodes that differ from the store.
#[derive(Default)]
pub(crate) struct Dirty(Mutex<Vec<Arc<Slot>>>);

impl Dirty {
    pub(crate) fn mark(&self, slot: &Arc<Slot>, node: &mut Node) {
        if !node.dirty {
            node.dirty = true;
            self.0.locked().push(slot.clone());
        }
    }

    /// Notes that the node was used, and queues the mark for the store when
    /// the stored one is old.
    pub(crate) fn touch(&self, slot: &Arc<Slot>, node: &mut Node) {
        node.used_at = now();
        if node.used_at.saturating_sub(node.synced_used_at) > TOUCH_AFTER {
            self.mark(slot, node);
        }
    }

    pub(crate) fn take(&self) -> Vec<Arc<Slot>> {
        std::mem::take(&mut *self.0.locked())
    }

    pub(crate) fn restore(&self, slots: Vec<Arc<Slot>>) {
        self.0.locked().extend(slots);
    }
}
