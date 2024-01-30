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

pub struct Registry<T>(RwLock<HashMap<u64, T>>);

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
        self.0.write().unwrap().remove(&handle.id)
    }
}

impl<T> Registry<T>
where
    T: Clone,
{
    pub fn collect(&self) -> Vec<T> {
        self.0.read().unwrap().values().cloned().collect()
    }

    pub fn get(&self, handle: Handle<T>) -> Result<T, InvalidHandle> {
        self.0
            .read()
            .unwrap()
            .get(&handle.id)
            .cloned()
            .ok_or(InvalidHandle)
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
            .insert(self.handle.id, value);
        self.handle
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

#[derive(Debug)]
pub struct InvalidHandle;

impl fmt::Display for InvalidHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid handle")
    }
}

impl std::error::Error for InvalidHandle {}
