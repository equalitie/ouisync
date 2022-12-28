pub mod tracing_layer;

use crate::sync::uninitialized_watch;
use serde::{
    ser::{SerializeMap, SerializeStruct},
    Serialize, Serializer,
};
use std::{
    collections::{btree_map, BTreeMap},
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

pub struct StateMonitor {
    shared: Arc<StateMonitorShared>,
}

struct StateMonitorShared {
    id: MonitorId,
    parent: Option<StateMonitor>,
    inner: Mutex<StateMonitorInner>,
}

struct StateMonitorInner {
    // Incremented on each change, can be used by monitors to determine whether a child has
    // changed.
    version: u64,
    // We need to keep track of the refcount ourselves instead of relying on the `Arc`'s
    // `strong_count`. The reason is that in `Drop` we remove `self` from the parent, if we did
    // this removal from insde `StateMonitorShared`s `drop` function, we could end up with the
    // parent pointing to no longer existing entry*.
    //
    // (*) Because the `Arc` first decrements it's strong count and only then destroys its value.
    // That means there is a time when the parent's weak ptr to the child is no longer valid.
    refcount: usize,
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
        self.make_non_unique_child(name, 0)
    }

    /// Use if we want to allow nodes of the same name.
    pub fn make_non_unique_child<S: Into<String>>(&self, name: S, disambiguator: u64) -> Self {
        let child_id = MonitorId {
            name: name.into(),
            disambiguator,
        };

        let mut lock = self.shared.lock_inner();
        let mut is_new = false;

        let child = match lock.children.entry(child_id.clone()) {
            btree_map::Entry::Vacant(e) => {
                is_new = true;

                let child = Arc::new(StateMonitorShared {
                    id: child_id,
                    parent: Some(Self {
                        shared: self.shared.clone(),
                    }),
                    inner: Mutex::new(StateMonitorInner {
                        version: 0,
                        refcount: 1,
                        values: BTreeMap::new(),
                        children: BTreeMap::new(),
                        on_change: uninitialized_watch::channel().0,
                    }),
                });

                e.insert(Arc::downgrade(&child));
                child
            }
            btree_map::Entry::Occupied(e) => {
                // Unwrap OK because children are responsible for removing themselves from the map
                // on Drop.
                e.get().upgrade().unwrap()
            }
        };

        if is_new {
            lock.refcount += 1;
            self.shared.changed(lock);
        }

        Self { shared: child }
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
        let mut lock = self.shared.lock_inner();
        let value = Arc::new(Mutex::new(value));

        match lock.values.entry(name.clone()) {
            btree_map::Entry::Vacant(e) => {
                e.insert(MonitoredValueHandle {
                    refcount: 1,
                    ptr: value.clone(),
                });
            }
            btree_map::Entry::Occupied(mut e) => {
                tracing::error!(
                    "StateMonitor: Monitored value of the same name ({:?}) under monitor {:?} already exists",
                    name,
                    self.shared.path()
                );
                let v = e.get_mut();
                v.refcount += 1;
                v.ptr = Arc::new(Mutex::new("<AMBIGUOUS>"));
            }
        };

        lock.refcount += 1;

        self.shared.changed(lock);

        MonitoredValue {
            name,
            monitor: Self {
                shared: self.shared.clone(),
            },
            value,
        }
    }

    /// Get notified whenever there is a change in this StateMonitor
    pub fn subscribe(&self) -> uninitialized_watch::Receiver<()> {
        self.shared.subscribe()
    }
}

impl Clone for StateMonitor {
    fn clone(&self) -> Self {
        let mut inner = self.shared.lock_inner();
        inner.refcount += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for StateMonitor {
    fn drop(&mut self) {
        let mut inner = self.shared.lock_inner();

        inner.refcount -= 1;

        if inner.refcount != 0 {
            return;
        }

        drop(inner);

        // This is the last instance of this state monitor. If it's not the root, remove it from
        // the parent.
        if let Some(parent) = &self.shared.parent {
            let id = self.shared.id.clone();
            let mut parent_lock = parent.shared.lock_inner();

            if let btree_map::Entry::Occupied(e) = parent_lock.children.entry(id) {
                e.remove();
                parent.shared.changed(parent_lock);
            }
        }
    }
}

impl StateMonitorShared {
    fn make_root() -> Arc<Self> {
        Arc::new(StateMonitorShared {
            id: MonitorId::root(),
            parent: None,
            inner: Mutex::new(StateMonitorInner {
                version: 0,
                refcount: 1,
                values: BTreeMap::new(),
                children: BTreeMap::new(),
                on_change: uninitialized_watch::channel().0,
            }),
        })
    }

    fn locate<I: Iterator<Item = MonitorId>>(self: &Arc<Self>, mut path: I) -> Option<Arc<Self>> {
        let child_id = match path.next() {
            Some(child_id) => child_id,
            None => return Some(self.clone()),
        };

        // Note: it can still happen that an entry exists in the map but it's refcount is zero.
        // See the comment in `make_child` for more details.
        self.lock_inner()
            .children
            .get(&child_id)
            .and_then(|child| child.upgrade())
            .and_then(|child| child.locate(path))
    }

    fn subscribe(self: &Arc<Self>) -> uninitialized_watch::Receiver<()> {
        self.lock_inner().on_change.subscribe()
    }

    fn changed(&self, mut lock: MutexGuard<'_, StateMonitorInner>) {
        lock.version += 1;
        lock.on_change.send(()).unwrap_or(());

        // When serializing, we lock from parent to child (to access the child's `version`), so
        // make sure we don't try to lock in the reverse direction as that could deadlock.
        drop(lock);

        if let Some(parent) = &self.parent {
            parent.shared.changed(parent.shared.lock_inner());
        }
    }

    fn lock_inner(&self) -> MutexGuard<'_, StateMonitorInner> {
        self.inner.lock().unwrap()
    }

    fn path(&self) -> String {
        if let Some(parent) = self.parent.as_ref().map(|parent| &parent.shared) {
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

// --- MonitoredValue

pub struct MonitoredValue<T> {
    name: String,
    monitor: StateMonitor,
    value: Arc<Mutex<T>>,
}

impl<T> Clone for MonitoredValue<T> {
    fn clone(&self) -> Self {
        let mut lock = self.monitor.shared.lock_inner();

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
    monitor: StateMonitor,
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
        self.monitor
            .shared
            .changed(self.monitor.shared.lock_inner());
    }
}

impl<T> Drop for MonitoredValue<T> {
    fn drop(&mut self) {
        let mut lock = self.monitor.shared.lock_inner();

        // Can we avoid cloning `self.name` (since we're droping anyway)?
        match lock.values.entry(self.name.clone()) {
            btree_map::Entry::Occupied(mut e) => {
                let v = e.get_mut();
                v.refcount -= 1;
                if v.refcount == 0 {
                    e.remove();
                    self.monitor.shared.changed(lock);
                }
            }
            btree_map::Entry::Vacant(_) => unreachable!(),
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
        let lock = self.shared.lock_inner();

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
            map.serialize_entry(k, &v.ptr.lock().unwrap().to_string())?;
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
            map.serialize_entry(
                &format!("{}:{}", id.disambiguator, id.name),
                &child.upgrade().unwrap().lock_inner().version,
            )?;
        }
        map.end()
    }
}

// ---
