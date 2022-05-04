use std::{
    collections::{btree_map as map, BTreeMap},
    fmt,
    ops::Drop,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        Weak,
        Mutex,
        MutexGuard,
    },
};
use slab::Slab;

pub struct StateMonitor {
    // The `id` is only unique within the parent.
    id: u64,
    name: String,
    parent: Weak<StateMonitor>,
    values: Mutex<BTreeMap<String, Slab<MonitoredValueHandle>>>,
    children: Mutex<BTreeMap<String, Arc<StateMonitor>>>,
    on_change: Mutex<Slab<Box<dyn FnMut()>>>,
    next_child_id: AtomicU64,
}

impl StateMonitor {
    pub fn make_root() -> Arc<Self> {
        Arc::new(Self {
            id: 0,
            name: "".into(),
            parent: Weak::new(),
            values: Mutex::new(BTreeMap::new()),
            children: Mutex::new(BTreeMap::new()),
            on_change: Mutex::new(Slab::new()),
            next_child_id: AtomicU64::new(0),
        })
    }

    pub fn make_child(self: &Arc<Self>, name: String) -> Arc<StateMonitor> {
        let weak_self = Arc::downgrade(self);
        let mut is_new = false;
        let name_clone = name.clone();

        let child = self.children.lock().unwrap().entry(name_clone).or_insert_with(|| {
            is_new = true;
            let id = self.next_child_id.fetch_add(1, Ordering::Relaxed);

            Arc::new(Self {
                id,
                name,
                parent: weak_self,
                values: Mutex::new(BTreeMap::new()),
                children: Mutex::new(BTreeMap::new()),
                on_change: Mutex::new(Slab::new()),
                next_child_id: AtomicU64::new(0),
            })
        }).clone();

        if is_new {
            self.changed();
        }

        child
    }

    pub fn make_value<T: 'static + fmt::Debug>(self: &Arc<Self>, key: String, value: T) -> MonitoredValue<T> {
        let value = Arc::new(Mutex::new(value));

        let id = match self.values.lock().unwrap().entry(key.clone()) {
            map::Entry::Vacant(e) => {
                let mut slab = Slab::new();

                let id = slab.insert(MonitoredValueHandle {
                    ptr: value.clone(),
                });

                e.insert(slab);

                id
            },
            map::Entry::Occupied(mut e) => {
                e.get_mut().insert(MonitoredValueHandle {
                    ptr: value.clone(),
                })
            }
        };

        self.changed();

        MonitoredValue {
            id,
            name: key,
            monitor: Arc::downgrade(self),
            value,
        }
    }

    pub fn children(&self) -> Vec<String> {
        self.children.lock().unwrap().keys().cloned().collect()
    }

    pub fn child(&self, name: &str) -> Option<Arc<StateMonitor>> {
        self.children.lock().unwrap().get(name).cloned()
    }

    pub fn on_change<F: 'static + FnMut()>(self: &Arc<Self>, f: F) -> OnChangeHandle {
        let weak_self = Arc::downgrade(self);
        let mut on_change = self.on_change.lock().unwrap();
        let handle = on_change.insert(Box::new(f));

        OnChangeHandle {
            state_monitor: weak_self,
            handle
        }
    }

    fn changed(&self) {
        // The documentation suggests not to iterate over slabs as it may be inefficient, but we
        // expect there will be very few handlers inside `on_change` so it shouldn't matter.
        for p in self.on_change.lock().unwrap().iter_mut() {
            p.1();
        }

        if let Some(parent) = self.parent.upgrade() {
            parent.changed();
        }
    }
}

impl Drop for StateMonitor {
    fn drop(&mut self) {
        if let Some(parent) = self.parent.upgrade() {
            let name = self.name.clone();
            let mut children = parent.children.lock().unwrap();

            if let map::Entry::Occupied(e) = children.entry(name) {
                if e.get().id == self.id {
                    e.remove();
                    drop(children);
                    parent.changed();
                }
            }
        }
    }
}

pub struct MonitoredValue<T> {
    id: usize,
    name: String,
    monitor: Weak<StateMonitor>,
    value: Arc<Mutex<T>>,
}

impl<T> MonitoredValue<T> {
    pub fn get(&self) -> MutexGuard<'_, T> {
        self.value.lock().unwrap()
    }

    pub fn set(&mut self, value: T) {
        *self.value.lock().unwrap() = value;
        if let Some(monitor) = self.monitor.upgrade() {
            monitor.changed();
        }
    }
}

impl<T> Drop for MonitoredValue<T> {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.upgrade() {
            let mut values = monitor.values.lock().unwrap();
            // TODO: Can we do without cloning the name? Since we're `drop`ing here anyway...
            if let map::Entry::Occupied(mut e) = values.entry(self.name.clone()) {
                e.get_mut().remove(self.id);
                drop(values);
                monitor.changed();
            }
        }
    }
}

struct MonitoredValueHandle {
    ptr: Arc<Mutex<dyn fmt::Debug>>,
}

pub struct OnChangeHandle {
    state_monitor: Weak<StateMonitor>,
    handle: usize,
}

impl Drop for OnChangeHandle {
    fn drop(&mut self) {
        if let Some(state_monitor) = self.state_monitor.upgrade() {
            state_monitor.on_change.lock().unwrap().remove(self.handle);
            state_monitor.changed();
        }
    }
}
