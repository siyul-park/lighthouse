use std::{
    any::{Any, TypeId},
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
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

/// The files a rule still has to be run for in this run, by rule id: the ones
/// whose stored findings the cache did not have. A rule that runs a program
/// over a batch of files runs it over these alone. A rule with no entry has no
/// such list (nothing was looked up), and is wanted everywhere.
#[derive(Default)]
pub struct Pending(Mutex<HashMap<String, BTreeSet<PathBuf>>>);

impl Pending {
    /// Notes that `rule` has to be run for `file`.
    pub fn add(&self, rule: &str, file: &Path) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(rule.to_owned())
            .or_default()
            .insert(file.to_owned());
    }

    /// Whether `rule` has to be run for `file`.
    pub fn wants(&self, rule: &str, file: &Path) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(rule)
            .is_none_or(|files| files.contains(file))
    }
}
