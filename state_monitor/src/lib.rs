pub mod metrics;
#[cfg(test)]
mod tests;

use deadlock::{BlockingMutex, BlockingMutexGuard};
use indexmap::{map::Entry, IndexMap};
use serde::{
    de::Error as _,
    ser::{SerializeMap, SerializeSeq, SerializeStruct},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{
    any::Any,
    convert::Into,
    fmt,
    ops::Drop,
    str::FromStr,
    sync::{Arc, Weak},
};
use tokio::sync::watch;

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
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

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for MonitorId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.name, self.disambiguator)
    }
}

impl FromStr for MonitorId {
    type Err = MonitorIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if let Some(index) = s.rfind(':') {
            let disambiguator = &s[index + 1..];
            let disambiguator = disambiguator.parse().map_err(|_| MonitorIdParseError)?;

            Ok(Self {
                name: s[..index].to_owned(),
                disambiguator,
            })
        } else {
            Ok(Self {
                name: s.to_owned(),
                disambiguator: 0,
            })
        }
    }
}

impl Serialize for MonitorId {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_string().serialize(s)
    }
}

impl<'de> Deserialize<'de> for MonitorId {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <&str>::deserialize(d)?;
        s.parse().map_err(D::Error::custom)
    }
}

#[derive(Debug)]
pub struct MonitorIdParseError;

impl fmt::Display for MonitorIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to parse MonitorId")
    }
}

// --- StateMonitor

pub struct StateMonitor {
    shared: Arc<StateMonitorShared>,
}

struct StateMonitorShared {
    id: MonitorId,
    parent: Option<StateMonitor>,
    inner: BlockingMutex<StateMonitorInner>,
    on_change: watch::Sender<()>,
}

struct StateMonitorInner {
    values: IndexMap<String, MonitoredValueHandle>,
    children: IndexMap<MonitorId, ChildEntry>,
}

struct ChildEntry {
    // We need to keep track of the refcount ourselves instead of relying on the `Arc`'s
    // `strong_count`. The reason is that in `Drop` we remove `self` from the parent, if we did
    // this removal from insde `StateMonitorShared`s `drop` function, we could end up with the
    // parent pointing to no longer existing entry*.
    //
    // (*) Because the `Arc` first decrements it's strong count and only then destroys its value.
    // That means there is a time when the parent's weak ptr to the child is no longer valid.
    //
    // Also note that we keep the `refcount` for a `StateMonitor` in it's parent. It may be
    // slightly inefficient (because of the added Map lookup), but allows us to lock only the
    // parent when decrementing it. I.e. if we had `refcount` inside the involved `StateMonitor` we
    // would need to (1) lock the parent, (2) lock self, (3) decrement refcount and finally (4)
    // remove self from parent if refcount decreased to zero. Locking the parent and self at the
    // same time (steps #1 and #2) could lead to a deadlock.
    refcount: usize,
    child: Weak<StateMonitorShared>,
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
            Entry::Vacant(e) => {
                is_new = true;

                let child = Arc::new(StateMonitorShared {
                    id: child_id,
                    // We can't do `self.clone()` here because cloning calls `lock_inner` and thus
                    // we'd deadlock. We'll increment our `refcount` further down this function.
                    parent: Some(Self {
                        shared: self.shared.clone(),
                    }),
                    inner: BlockingMutex::new(StateMonitorInner {
                        values: Default::default(),
                        children: Default::default(),
                    }),
                    on_change: watch::channel(()).0,
                });

                e.insert(ChildEntry {
                    refcount: 1,
                    child: Arc::downgrade(&child),
                });
                child
            }
            Entry::Occupied(mut e) => {
                e.get_mut().refcount += 1;
                // Unwrap OK because children are responsible for removing themselves from the map
                // on Drop.
                e.get().child.upgrade().unwrap()
            }
        };

        drop(lock);

        if is_new {
            // We "cloned" `self` in the `parent` field above so need to increment `refcount`.
            // Note that it's OK to do the `refcount` increment here as opposed to at the beginning
            // of this function because given that `self` exists it must be that `refcount` doesn't
            // drop to zero anywhere in this function (and thus won't be removed from parent).
            self.shared.increment_refcount();
            self.shared.changed();
        }

        Self { shared: child }
    }

    pub fn id(&self) -> &MonitorId {
        &self.shared.id
    }

    pub fn locate<I: IntoIterator<Item = MonitorId>>(&self, path: I) -> Option<Self> {
        self.shared.locate(path).map(|shared| {
            shared.increment_refcount();
            Self { shared }
        })
    }

    /// Creates a new monitored value. The caller is responsible for ensuring that there is always
    /// at most one value of a given `name` per StateMonitor instance.
    ///
    /// If the caller fails to ensure this uniqueness, the value of this variable shall be seen as
    /// the string "<AMBIGUOUS>". Such solution seem to be more sensible than panicking given that
    /// this is only a monitoring piece of code.
    pub fn make_value<N: Into<String>, T: Value>(&self, name: N, value: T) -> MonitoredValue<T> {
        let mut lock = self.shared.lock_inner();

        let name = name.into();
        let value = Arc::new(BlockingMutex::new(value));

        match lock.values.entry(name.clone()) {
            Entry::Vacant(e) => {
                e.insert(MonitoredValueHandle {
                    refcount: 1,
                    ptr: value.clone(),
                });
            }
            Entry::Occupied(mut e) => {
                tracing::error!(
                    "StateMonitor: Monitored value of the same name ({:?}) under monitor {:?} already exists",
                    name,
                    self.shared.path()
                );
                let v = e.get_mut();
                v.refcount += 1;
                v.ptr = Arc::new(BlockingMutex::new("<AMBIGUOUS>"));
            }
        };

        drop(lock); // Drop ASAP

        self.shared.changed();

        MonitoredValue {
            name,
            monitor: self.clone(),
            value,
        }
    }

    /// Gets current snapshot of the given value.
    pub fn get_value<T>(&self, name: &str) -> Result<T, ValueError>
    where
        T: Any + Clone,
    {
        Ok(self
            .shared
            .lock_inner()
            .values
            .get(name)
            .ok_or(ValueError::NotFound)?
            .ptr
            .lock()
            .unwrap()
            .as_any()
            .downcast_ref::<T>()
            .ok_or(ValueError::WrongType)?
            .clone())
    }

    /// Get notified whenever there is a change in this StateMonitor
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.shared.subscribe()
    }
}

impl Clone for StateMonitor {
    fn clone(&self) -> Self {
        self.shared.increment_refcount();
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for StateMonitor {
    fn drop(&mut self) {
        let parent = match &self.shared.parent {
            Some(parent) => parent,
            None => return,
        };

        let mut parent_inner = parent.shared.lock_inner();

        let mut entry = match parent_inner.children.entry(self.shared.id.clone()) {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(_) => unreachable!(),
        };

        let refcount = &mut entry.get_mut().refcount;

        *refcount -= 1;

        if *refcount != 0 {
            return;
        }

        entry.shift_remove();
        parent.shared.changed();
    }
}

// These impls are needed only for the tests to compile.
impl PartialEq for StateMonitor {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Eq for StateMonitor {}

impl StateMonitorShared {
    fn make_root() -> Arc<Self> {
        Arc::new(StateMonitorShared {
            id: MonitorId::root(),
            parent: None,
            inner: BlockingMutex::new(StateMonitorInner {
                values: Default::default(),
                children: Default::default(),
            }),
            on_change: watch::channel(()).0,
        })
    }

    fn locate<I: IntoIterator<Item = MonitorId>>(self: &Arc<Self>, path: I) -> Option<Arc<Self>> {
        let mut path = path.into_iter();
        let child_id = match path.next() {
            Some(child_id) => child_id,
            None => return Some(self.clone()),
        };

        let child = self
            .lock_inner()
            .children
            .get(&child_id)
            .map(|entry| entry.child.upgrade().unwrap());

        // Don't inline this with the previous command because we need to unlock inner before
        // recursing to the child.
        child.and_then(|child| child.locate(path))
    }

    fn subscribe(self: &Arc<Self>) -> watch::Receiver<()> {
        self.on_change.subscribe()
    }

    fn changed(&self) {
        self.on_change.send(()).unwrap_or(());
    }

    fn lock_inner(&self) -> BlockingMutexGuard<'_, StateMonitorInner> {
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

    fn increment_refcount(&self) {
        if let Some(parent) = self.parent.as_ref().map(|parent| &parent.shared) {
            parent
                .lock_inner()
                .children
                .get_mut(&self.id)
                .unwrap()
                .refcount += 1;
        }
    }
}

// --- MonitoredValue

pub struct MonitoredValue<T> {
    name: String,
    monitor: StateMonitor,
    value: Arc<BlockingMutex<T>>,
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
            guard: self.value.lock().unwrap(),
        }
    }
}

pub struct MutexGuardWrap<'a, T> {
    monitor: StateMonitor,
    guard: BlockingMutexGuard<'a, T>,
}

impl<T> core::ops::Deref for MutexGuardWrap<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> core::ops::DerefMut for MutexGuardWrap<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for MutexGuardWrap<'_, T> {
    fn drop(&mut self) {
        self.monitor.shared.changed();
    }
}

impl<T> Drop for MonitoredValue<T> {
    fn drop(&mut self) {
        let mut lock = self.monitor.shared.lock_inner();

        // Can we avoid cloning `self.name` (since we're droping anyway)?
        match lock.values.entry(self.name.clone()) {
            Entry::Occupied(mut e) => {
                let v = e.get_mut();
                v.refcount -= 1;
                if v.refcount == 0 {
                    e.shift_remove();
                    self.monitor.shared.changed();
                }
            }
            Entry::Vacant(_) => unreachable!(),
        }
    }
}

struct MonitoredValueHandle {
    refcount: usize,
    ptr: Arc<BlockingMutex<dyn Value>>,
}

pub trait Value: fmt::Debug + Any + Send + 'static {
    fn as_any(&self) -> &dyn Any;
}

impl<T> Value for T
where
    T: fmt::Debug + Any + Send + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Eq, PartialEq, Debug)]
pub enum ValueError {
    NotFound,
    WrongType,
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "value not found"),
            Self::WrongType => write!(f, "value has wrong type"),
        }
    }
}

impl std::error::Error for ValueError {}

// --- Serialization

impl fmt::Debug for StateMonitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateMonitor").finish_non_exhaustive()
    }
}

impl Serialize for StateMonitor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let lock = self.shared.lock_inner();

        // When serializing into the messagepack format, the `serialize_struct(_, N)` is serialized
        // into a list of size N (use `unpackList` in Dart).
        let mut s = serializer.serialize_struct("StateMonitor", 2)?;
        s.serialize_field("values", &ValuesSerializer(&lock.values))?;
        s.serialize_field("children", &ChildrenSerializer(&lock.children))?;
        s.end()
    }
}

struct ValuesSerializer<'a>(&'a IndexMap<String, MonitoredValueHandle>);
struct ChildrenSerializer<'a>(&'a IndexMap<MonitorId, ChildEntry>);

impl Serialize for ValuesSerializer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0.iter() {
            let value = format!("{:?}", &*v.ptr.lock().unwrap());
            map.serialize_entry(k, &value)?;
        }
        map.end()
    }
}

impl Serialize for ChildrenSerializer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_seq(Some(self.0.len()))?;
        for id in self.0.keys() {
            map.serialize_element(&id.to_string())?;
        }
        map.end()
    }
}

// TODO: Implement Deserialize for StateMonitor
impl<'de> Deserialize<'de> for StateMonitor {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        todo!()
    }
}

// ---
