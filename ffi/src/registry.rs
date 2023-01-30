use crate::dart::DartCObject;
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

pub(crate) struct Registry<T>(RwLock<HashMap<u64, Arc<T>>>);

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

pub(crate) struct VacantEntry<'a, T>
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

    pub(crate) fn id(&self) -> u64 {
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

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct NullableHandle<T: 'static>(Handle<T>);

impl<T> NullableHandle<T> {
    pub(crate) const NULL: Self = Self(Handle {
        id: 0,
        _type: PhantomData,
    });
}

impl<T> From<Handle<T>> for NullableHandle<T> {
    fn from(handle: Handle<T>) -> Self {
        Self(handle)
    }
}

impl<T> TryFrom<NullableHandle<T>> for Handle<T> {
    type Error = NullError;

    fn try_from(handle: NullableHandle<T>) -> Result<Self, Self::Error> {
        if handle.0.id == 0 {
            Err(NullError)
        } else {
            Ok(handle.0)
        }
    }
}

pub struct NullError;

impl<T> From<NullableHandle<T>> for DartCObject {
    fn from(handle: NullableHandle<T>) -> Self {
        DartCObject::from(handle.0)
    }
}

impl<T> From<Handle<T>> for DartCObject {
    fn from(handle: Handle<T>) -> Self {
        DartCObject::from(handle.id)
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
