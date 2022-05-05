use crate::sync::uninitialized_watch;
use serde::{
    ser::{SerializeMap, SerializeStruct},
    Serialize, Serializer,
};
use std::{
    collections::{btree_map as map, BTreeMap},
    fmt,
    ops::Drop,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, Weak,
    },
};

static NEXT_MONITOR_ID: AtomicU64 = AtomicU64::new(0);

// --- StateMonitor

pub struct StateMonitor {
    id: u64,
    name: String,
    parent: Weak<StateMonitor>,
    inner: Mutex<StateMonitorInner>,
}

pub struct StateMonitorInner {
    // Incremented on each change, can be used by monitors to determine whether a child has
    // changed.
    change_id: u64,
    values: BTreeMap<String, MonitoredValueHandle>,
    children: BTreeMap<String, Arc<StateMonitor>>,
    on_change: uninitialized_watch::Sender<()>,
}

impl StateMonitor {
    pub fn make_root() -> Arc<Self> {
        Arc::new(Self {
            id: 0,
            name: "".into(),
            parent: Weak::new(),
            inner: Mutex::new(StateMonitorInner {
                change_id: 0,
                values: BTreeMap::new(),
                children: BTreeMap::new(),
                on_change: uninitialized_watch::channel().0,
            }),
        })
    }

    pub fn make_child(self: &Arc<Self>, name: String) -> Arc<StateMonitor> {
        let weak_self = Arc::downgrade(self);
        let mut is_new = false;
        let name_clone = name.clone();
        let mut lock = self.lock();

        let child = lock
            .children
            .entry(name_clone)
            .or_insert_with(|| {
                is_new = true;
                let id = NEXT_MONITOR_ID.fetch_add(1, Ordering::Relaxed);

                Arc::new(Self {
                    id,
                    name,
                    parent: weak_self,
                    inner: Mutex::new(StateMonitorInner {
                        change_id: 0,
                        values: BTreeMap::new(),
                        children: BTreeMap::new(),
                        on_change: uninitialized_watch::channel().0,
                    }),
                })
            })
            .clone();

        if is_new {
            self.changed(lock);
        }

        child
    }

    pub fn locate(self: &Arc<Self>, path: &str) -> Option<Arc<Self>> {
        if path.is_empty() {
            return Some(self.clone());
        }

        let (child, rest) = match path.find(':') {
            Some(split_at) => {
                let (child, rest) = path.split_at(split_at);
                (child, Some(&rest[1..]))
            }
            None => (path, None),
        };

        let lock = self.lock();

        lock.children.get(child).and_then(|child| match rest {
            Some(rest) => child.locate(rest),
            None => Some(child.clone()),
        })
    }

    /// Creates a new monitored value. The caller is responsible for ensuring that there is always
    /// at most one value of a given `name` per StateMonitor instance.
    ///
    /// If the caller fails to ensure this uniqueness, the value of this variable shall be seen as
    /// the string "<AMBIGUOUS>". Such solution seem to be more sensible than panicking given that
    /// this is only a monitoring piece of code.
    pub fn make_value<T: 'static + fmt::Debug + Sync + Send>(
        self: &Arc<Self>,
        name: String,
        value: T,
    ) -> MonitoredValue<T> {
        let value = Arc::new(Mutex::new(value));
        let mut lock = self.lock();

        match lock.values.entry(name.clone()) {
            map::Entry::Vacant(e) => {
                e.insert(MonitoredValueHandle {
                    refcount: 1,
                    ptr: value.clone(),
                });
            }
            map::Entry::Occupied(mut e) => {
                log::error!(
                    "StateMonitor: Monitored value of the same name ({:?}) already exists",
                    name
                );
                let v = e.get_mut();
                v.refcount += 1;
                v.ptr = Arc::new(Mutex::new("<AMBIGUOUS>"));
            }
        };

        self.changed(lock);

        MonitoredValue {
            name,
            monitor: Arc::downgrade(self),
            value,
        }
    }

    /// Get notified whenever there is a change in this StateMonitor
    pub fn subscribe(self: &Arc<Self>) -> uninitialized_watch::Receiver<()> {
        self.lock().on_change.subscribe()
    }

    fn changed(&self, mut lock: MutexGuard<'_, StateMonitorInner>) {
        lock.change_id += 1;
        lock.on_change.send(()).unwrap_or(());

        // When serializing, we lock from parent to child (to access the child's `change_id`), so
        // make sure we don't try to lock in the reverse direction as that could deadlock.
        drop(lock);

        if let Some(parent) = self.parent.upgrade() {
            parent.changed(parent.lock());
        }
    }

    fn lock(&self) -> MutexGuard<'_, StateMonitorInner> {
        self.inner.lock().unwrap()
    }
}

impl Drop for StateMonitor {
    fn drop(&mut self) {
        if let Some(parent) = self.parent.upgrade() {
            let name = self.name.clone();
            let mut parent_lock = parent.lock();

            if let map::Entry::Occupied(e) = parent_lock.children.entry(name) {
                if e.get().id == self.id {
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
    monitor: Weak<StateMonitor>,
    value: Arc<Mutex<T>>,
}

impl<T> MonitoredValue<T> {
    pub fn get(&self) -> MutexGuard<'_, T> {
        self.value.lock().unwrap()
    }

    pub fn set(&self, value: T) {
        *self.value.lock().unwrap() = value;
        if let Some(monitor) = self.monitor.upgrade() {
            monitor.changed(monitor.lock());
        }
    }
}

impl<T> Drop for MonitoredValue<T> {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.upgrade() {
            let mut lock = monitor.lock();

            // Can we not clone (since we're droping anyway)?
            match lock.values.entry(self.name.clone()) {
                map::Entry::Occupied(mut e) => {
                    let v = e.get_mut();
                    v.refcount -= 1;
                    if v.refcount == 0 {
                        e.remove();
                        monitor.changed(lock);
                    }
                }
                map::Entry::Vacant(_) => unreachable!(),
            }
        }
    }
}

struct MonitoredValueHandle {
    refcount: usize,
    ptr: Arc<Mutex<dyn fmt::Debug + Sync + Send>>,
}

// --- Serialization

impl Serialize for StateMonitor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let lock = self.lock();

        // When serializing into the messagepack format, the `serialize_struct(_, N)` is serialized
        // into a list of size N (use `unpackList` in Dart).
        let mut s = serializer.serialize_struct("StateMonitor", 3)?;
        s.serialize_field("change_id", &lock.change_id)?;
        s.serialize_field("values", &ValuesSerializer(&lock.values))?;
        s.serialize_field("children", &ChildrenSerializer(&lock.children))?;
        s.end()
    }
}

struct ValuesSerializer<'a>(&'a BTreeMap<String, MonitoredValueHandle>);
struct ChildrenSerializer<'a>(&'a BTreeMap<String, Arc<StateMonitor>>);

impl<'a> Serialize for ValuesSerializer<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0.iter() {
            let v = v.ptr.lock().unwrap();
            map.serialize_entry(k, &format!("{:?}", v))?;
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
        for (name, child) in self.0.iter() {
            map.serialize_entry(name, &child.lock().change_id)?;
        }
        map.end()
    }
}

// ---
