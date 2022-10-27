pub mod tracing_layer;

use crate::sync::uninitialized_watch;
use serde::{
    ser::{SerializeMap, SerializeStruct},
    Serialize, Serializer,
};
use std::{
    collections::{btree_map as map, BTreeMap},
    convert::Into,
    fmt,
    ops::Drop,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

#[derive(Debug, Eq, PartialEq, PartialOrd, Ord, Clone)]
pub struct MonitorId {
    name: String,
    disambiguator: u64,
}

impl MonitorId {
    fn root() -> Self {
        Self {
            name: "".into(),
            disambiguator: 0,
        }
    }

    pub fn new(name: String, disambiguator: u64) -> Self {
        Self {
            name,
            disambiguator,
        }
    }
}

// --- StateMonitor

#[derive(Clone)]
pub struct StateMonitor {
    shared: Arc<StateMonitorShared>,
}

struct StateMonitorShared {
    id: MonitorId,
    parent: Option<Arc<StateMonitorShared>>,
    inner: Mutex<StateMonitorInner>,
}

struct StateMonitorInner {
    // Incremented on each change, can be used by monitors to determine whether a child has
    // changed.
    version: u64,
    values: BTreeMap<String, MonitoredValueHandle>,
    children: BTreeMap<MonitorId, Weak<StateMonitorShared>>,
    on_change: uninitialized_watch::Sender<()>,
}

impl StateMonitor {
    pub fn make_root() -> Self {
        Self {
            shared: StateMonitorShared::make_root(),
        }
    }

    pub fn make_child<S: Into<String>>(&self, name: S) -> Self {
        let child_id = MonitorId {
            name: name.into(),
            disambiguator: 0,
        };

        Self {
            shared: self.shared.make_child(child_id),
        }
    }

    /// Use if we want to allow nodes of the same name.
    pub fn make_non_unique_child<S: Into<String>>(&self, name: S, disambiguator: u64) -> Self {
        let child_id = MonitorId {
            name: name.into(),
            disambiguator,
        };

        Self {
            shared: self.shared.make_child(child_id),
        }
    }

    pub fn locate<I: Iterator<Item = MonitorId>>(&self, path: I) -> Option<Self> {
        self.shared.locate(path).map(|shared| Self { shared })
    }

    /// Creates a new monitored value. The caller is responsible for ensuring that there is always
    /// at most one value of a given `name` per StateMonitor instance.
    ///
    /// If the caller fails to ensure this uniqueness, the value of this variable shall be seen as
    /// the string "<AMBIGUOUS>". Such solution seem to be more sensible than panicking given that
    /// this is only a monitoring piece of code.
    pub fn make_value<T: fmt::Display + Sync + Send + 'static>(
        &self,
        name: String,
        value: T,
    ) -> MonitoredValue<T> {
        self.shared.make_value(name, value)
    }

    /// Get notified whenever there is a change in this StateMonitor
    pub fn subscribe(&self) -> uninitialized_watch::Receiver<()> {
        self.shared.subscribe()
    }
}

impl StateMonitorShared {
    fn make_root() -> Arc<Self> {
        Arc::new(StateMonitorShared {
            id: MonitorId::root(),
            parent: None,
            inner: Mutex::new(StateMonitorInner {
                version: 0,
                values: BTreeMap::new(),
                children: BTreeMap::new(),
                on_change: uninitialized_watch::channel().0,
            }),
        })
    }

    fn make_child(self: &Arc<Self>, child_id: MonitorId) -> Arc<Self> {
        let mut lock = self.lock();
        let mut is_new = false;

        // Note: the nodes are responsible for removing themeselves from the map but it can still
        // happen the entry's refcount reaches zero before the entry itself is removed* and so we
        // have to handle also the case where the entry exists, but it's refcount is 0.
        //
        // *) because `Arc` decrements the refcount before running the destructor of the contained
        //    value.
        let child_weak = lock
            .children
            .entry(child_id.clone())
            .or_insert_with(Weak::new);
        let child = if let Some(child) = child_weak.upgrade() {
            child
        } else {
            let child = Arc::new(Self {
                id: child_id,
                parent: Some(self.clone()),
                inner: Mutex::new(StateMonitorInner {
                    version: 0,
                    values: BTreeMap::new(),
                    children: BTreeMap::new(),
                    on_change: uninitialized_watch::channel().0,
                }),
            });

            *child_weak = Arc::downgrade(&child);
            is_new = true;

            child
        };

        if is_new {
            self.changed(lock);
        }

        child
    }

    fn locate<I: Iterator<Item = MonitorId>>(self: &Arc<Self>, mut path: I) -> Option<Arc<Self>> {
        let child = match path.next() {
            Some(child) => child,
            None => return Some(self.clone()),
        };

        // Note: it can still happen that an entry exists in the map but it's refcount is zero.
        // See the comment in `make_child` for more details.
        self.lock()
            .children
            .get(&child)
            .and_then(|child| child.upgrade())
            .and_then(|child| child.locate(path))
    }

    fn make_value<T: fmt::Display + Sync + Send + 'static>(
        self: &Arc<Self>,
        name: String,
        value: T,
    ) -> MonitoredValue<T> {
        let mut lock = self.lock();
        let value = Arc::new(Mutex::new(value));

        match lock.values.entry(name.clone()) {
            map::Entry::Vacant(e) => {
                e.insert(MonitoredValueHandle {
                    refcount: 1,
                    ptr: value.clone(),
                });
            }
            map::Entry::Occupied(mut e) => {
                tracing::error!(
                    "StateMonitor: Monitored value of the same name ({:?}) under monitor {:?} already exists",
                    name,
                    self.path()
                );
                let v = e.get_mut();
                v.refcount += 1;
                v.ptr = Arc::new(Mutex::new("<AMBIGUOUS>"));
            }
        };

        self.changed(lock);

        MonitoredValue {
            name,
            monitor: self.clone(),
            value,
        }
    }

    fn subscribe(self: &Arc<Self>) -> uninitialized_watch::Receiver<()> {
        self.lock().on_change.subscribe()
    }

    fn changed(&self, mut lock: MutexGuard<'_, StateMonitorInner>) {
        lock.version += 1;
        lock.on_change.send(()).unwrap_or(());

        // When serializing, we lock from parent to child (to access the child's `version`), so
        // make sure we don't try to lock in the reverse direction as that could deadlock.
        drop(lock);

        if let Some(parent) = &self.parent {
            parent.changed(parent.lock());
        }
    }

    fn lock(&self) -> MutexGuard<'_, StateMonitorInner> {
        self.inner.lock().unwrap()
    }

    fn path(&self) -> String {
        if let Some(parent) = self.parent.as_ref() {
            format!(
                "{}/({},{})",
                parent.path(),
                self.id.name,
                self.id.disambiguator
            )
        } else {
            format!("/({},{})", self.id.name, self.id.disambiguator)
        }
    }
}

impl Drop for StateMonitorShared {
    fn drop(&mut self) {
        if let Some(parent) = &self.parent {
            let id = self.id.clone();
            let mut parent_lock = parent.lock();

            if let map::Entry::Occupied(e) = parent_lock.children.entry(id) {
                if e.get().strong_count() == 0 {
                    e.remove();
                    parent.changed(parent_lock);
                }
            }
        }
    }
}

// --- MonitoredValue

pub struct MonitoredValue<T> {
    name: String,
    monitor: Arc<StateMonitorShared>,
    value: Arc<Mutex<T>>,
}

impl<T> Clone for MonitoredValue<T> {
    fn clone(&self) -> Self {
        let mut lock = self.monitor.lock();

        // Unwrap OK because since this instance exists, there must be an entry for it in the
        // parent monitor.values map.
        lock.values.get_mut(&self.name).unwrap().refcount += 1;

        Self {
            name: self.name.clone(),
            monitor: self.monitor.clone(),
            value: self.value.clone(),
        }
    }
}

impl<T> MonitoredValue<T> {
    pub fn get(&self) -> MutexGuardWrap<'_, T> {
        MutexGuardWrap {
            monitor: self.monitor.clone(),
            guard: Some(self.value.lock().unwrap()),
        }
    }
}

pub struct MutexGuardWrap<'a, T> {
    monitor: Arc<StateMonitorShared>,
    // This is only None in the destructor.
    guard: Option<MutexGuard<'a, T>>,
}

impl<'a, T> core::ops::Deref for MutexGuardWrap<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.guard.as_ref().unwrap()
    }
}

impl<'a, T> core::ops::DerefMut for MutexGuardWrap<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut *(self.guard.as_mut().unwrap())
    }
}

impl<'a, T> Drop for MutexGuardWrap<'a, T> {
    fn drop(&mut self) {
        {
            // Unlock this before we try to lock the parent monitor.
            self.guard.take();
        }
        self.monitor.changed(self.monitor.lock());
    }
}

impl<T> Drop for MonitoredValue<T> {
    fn drop(&mut self) {
        let mut lock = self.monitor.lock();

        // Can we avoid cloning `self.name` (since we're droping anyway)?
        match lock.values.entry(self.name.clone()) {
            map::Entry::Occupied(mut e) => {
                let v = e.get_mut();
                v.refcount -= 1;
                if v.refcount == 0 {
                    e.remove();
                    self.monitor.changed(lock);
                }
            }
            map::Entry::Vacant(_) => unreachable!(),
        }
    }
}

struct MonitoredValueHandle {
    refcount: usize,
    ptr: Arc<Mutex<dyn fmt::Display + Sync + Send>>,
}

// --- Serialization

impl Serialize for StateMonitor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let lock = self.shared.lock();

        // When serializing into the messagepack format, the `serialize_struct(_, N)` is serialized
        // into a list of size N (use `unpackList` in Dart).
        let mut s = serializer.serialize_struct("StateMonitor", 3)?;
        s.serialize_field("version", &lock.version)?;
        s.serialize_field("values", &ValuesSerializer(&lock.values))?;
        s.serialize_field("children", &ChildrenSerializer(&lock.children))?;
        s.end()
    }
}

struct ValuesSerializer<'a>(&'a BTreeMap<String, MonitoredValueHandle>);
struct ChildrenSerializer<'a>(&'a BTreeMap<MonitorId, Weak<StateMonitorShared>>);

impl<'a> Serialize for ValuesSerializer<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0.iter() {
            let v = v.ptr.lock().unwrap();
            map.serialize_entry(k, &v.to_string())?;
        }
        map.end()
    }
}

impl<'a> Serialize for ChildrenSerializer<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (id, child) in self.0.iter() {
            // Unwrap OK because children are responsible for removing themselves from the map on
            // Drop.
            map.serialize_entry(
                &format!("{}:{}", id.disambiguator, id.name),
                &child.upgrade().unwrap().lock().version,
            )?;
        }
        map.end()
    }
}

// ---
