use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

/// What the rules of one run share so that each computes a value once: one
/// slot per type, created empty on first use. A slot holds its own
/// synchronization, so rules running in parallel may fill it together.
#[derive(Default)]
pub struct Memo {
    slots: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Memo {
    /// The run's slot of type `T`.
    pub fn slot<T: Default + Send + Sync + 'static>(&self) -> Arc<T> {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = slots
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(T::default()));
        Arc::clone(slot)
            .downcast::<T>()
            .unwrap_or_else(|_| unreachable!("the slot of a type holds that type"))
    }
}
