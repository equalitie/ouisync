use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        RwLock,
    },
};
use thiserror::Error;

pub struct Registry<T: 'static>(HashMap<Handle<T>, T>);

impl<T: 'static> Registry<T> {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn insert(&mut self, value: T) -> Handle<T> {
        let handle = Handle::new();
        self.0.insert(handle, value);
        handle
    }

    pub fn remove(&mut self, handle: Handle<T>) -> Option<T> {
        self.0.remove(&handle)
    }

    pub fn remove_all(&mut self) -> Vec<T> {
        self.0.drain().map(|(_handle, value)| value).collect()
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.0.values()
    }

    pub fn get(&self, handle: Handle<T>) -> Result<&T, InvalidHandle> {
        self.0.get(&handle).ok_or(InvalidHandle)
    }
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SharedRegistry<T: 'static>(RwLock<Registry<T>>);

impl<T: 'static> SharedRegistry<T> {
    pub fn new() -> Self {
        Self(RwLock::new(Registry::new()))
    }

    pub fn insert(&self, item: T) -> Handle<T> {
        self.0.write().unwrap().insert(item)
    }

    pub fn remove(&self, handle: Handle<T>) -> Option<T> {
        self.0.write().unwrap().remove(handle)
    }
}

impl<T> SharedRegistry<T>
where
    T: Clone,
{
    pub fn get(&self, handle: Handle<T>) -> Result<T, InvalidHandle> {
        self.0
            .read()
            .unwrap()
            .get(handle)
            .cloned()
            .map(|value| value.clone())
    }
}

#[derive(Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Handle<T: 'static> {
    id: u64,
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

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for Handle<T> {}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Handle").field(&self.id).finish()
    }
}

#[derive(Debug, Error)]
#[error("invalid handle")]
pub struct InvalidHandle;
