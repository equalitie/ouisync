use crate::dart::DartCObject;
use std::{
    collections::HashMap,
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

    pub fn insert(&self, item: T) -> Handle<T> {
        let handle = Handle::new();
        self.0.write().unwrap().insert(handle.id, Arc::new(item));
        handle
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

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Handle<T: 'static> {
    id: u64,
    _type: PhantomData<&'static T>,
}

impl<T> Handle<T> {
    pub(crate) fn new() -> Self {
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
