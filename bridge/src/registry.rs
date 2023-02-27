use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};

pub struct Registry<T>(RwLock<HashMap<u64, Arc<T>>>);

impl<T> Registry<T> {
    pub fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
    }

    pub fn vacant_entry(&self) -> VacantEntry<T> {
        let handle = Handle::new();
        VacantEntry {
            registry: self,
            handle,
        }
    }

    pub fn insert(&self, item: T) -> Handle<T> {
        self.vacant_entry().insert(item)
    }

    pub fn remove(&self, handle: Handle<T>) -> Option<T> {
        let mut items = self.0.write().unwrap();

        let ptr = items.remove(&handle.id)?;

        match Arc::try_unwrap(ptr) {
            Ok(item) => Some(item),
            Err(ptr) => {
                items.insert(handle.id, ptr);
                None
            }
        }
    }

    pub fn get(&self, handle: Handle<T>) -> Arc<T> {
        self.try_get(handle).expect("invalid handle")
    }

    pub fn try_get(&self, handle: Handle<T>) -> Option<Arc<T>> {
        self.0.read().unwrap().get(&handle.id).cloned()
    }
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct VacantEntry<'a, T>
where
    T: 'static,
{
    registry: &'a Registry<T>,
    handle: Handle<T>,
}

impl<T> VacantEntry<'_, T> {
    pub fn handle(&self) -> Handle<T> {
        self.handle
    }

    pub fn insert(self, value: T) -> Handle<T> {
        self.registry
            .0
            .write()
            .unwrap()
            .insert(self.handle.id, Arc::new(value));
        self.handle
    }
}

#[derive(Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Handle<T: 'static> {
    id: u64,
    #[serde()]
    _type: PhantomData<&'static T>,
}

impl<T> Handle<T> {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        // Make sure `id` is never 0 as that is reserved for the `NULL` handle.
        let id = loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                break id;
            }
        };

        Self {
            id,
            _type: PhantomData,
        }
    }

    pub fn from_id(id: u64) -> Self {
        Self {
            id,
            _type: PhantomData,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Handle").field(&self.id).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove() {
        let registry = Registry::new();
        let handle = registry.insert(0);

        let item = registry.get(handle);
        assert_eq!(registry.remove(handle), None);

        drop(item);
        assert_eq!(registry.remove(handle), Some(0));
        assert_eq!(registry.remove(handle), None);
    }
}
