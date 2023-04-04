//! Simple blocking timer

use std::{
    collections::{btree_map, BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Condvar, Mutex,
    },
    time::{Duration, Instant},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub(super) type Id = u64;

pub(super) struct Timer<T> {
    inner: Mutex<Inner<T>>,
    notify: Condvar,
}

struct Inner<T> {
    deadlines: BTreeMap<Instant, BTreeSet<Id>>,
    payloads: BTreeMap<Id, Holder<T>>,
}

struct Holder<T> {
    payload: T,
    deadline: Instant,
}

impl<T> Timer<T> {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                deadlines: BTreeMap::new(),
                payloads: BTreeMap::new(),
            }),
            notify: Condvar::new(),
        }
    }

    pub fn schedule(&self, deadline: Instant, payload: T) -> Id {
        let mut inner = self.inner.lock().unwrap();

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        inner.payloads.insert(id, Holder { payload, deadline });
        inner.deadlines.entry(deadline).or_default().insert(id);

        self.notify.notify_all();

        id
    }

    pub fn cancel(&self, id: Id) -> Option<T> {
        let mut inner = self.inner.lock().unwrap();

        let holder = inner.payloads.remove(&id)?;

        let btree_map::Entry::Occupied(mut entry) = inner.deadlines.entry(holder.deadline) else {
            unreachable!();
        };

        entry.get_mut().remove(&id);
        if entry.get().is_empty() {
            entry.remove();
        }

        Some(holder.payload)
    }

    pub fn wait(&self) -> (Id, T) {
        loop {
            let inner = self.inner.lock().unwrap();
            let next_deadline = inner.deadlines.keys().next().copied();

            let mut inner = if let Some(deadline) = next_deadline {
                let timeout = deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::ZERO);
                self.notify.wait_timeout(inner, timeout).unwrap().0
            } else {
                self.notify.wait(inner).unwrap()
            };

            let Some(mut entry) = inner.deadlines.first_entry() else {
                continue;
            };

            if *entry.key() > Instant::now() {
                continue;
            }

            let Some(id) = entry.get_mut().pop_first() else {
                unreachable!();
            };

            if entry.get().is_empty() {
                entry.remove();
            }

            let Some(Holder { payload, ..}) = inner.payloads.remove(&id) else {
                unreachable!();
            };

            return (id, payload);
        }
    }

    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().deadlines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule() {
        let timer = Timer::new();

        let start = Instant::now();

        timer.schedule(Instant::now() + Duration::from_millis(100), 0);
        timer.schedule(Instant::now() + Duration::from_millis(50), 1);
        timer.schedule(Instant::now() + Duration::from_millis(150), 2);
        timer.schedule(Instant::now() + Duration::from_millis(150), 3);

        assert_eq!(timer.wait().1, 1);
        assert_eq!(timer.wait().1, 0);
        assert_eq!(timer.wait().1, 2);
        assert_eq!(timer.wait().1, 3);
        assert!(timer.is_empty());

        // we should be done in 150ms but need to account for clock inaccuracy.
        assert!(start.elapsed() > Duration::from_millis(120));
        assert!(start.elapsed() < Duration::from_millis(170));
    }

    #[test]
    fn cancel() {
        let timer = Timer::new();
        let token = timer.schedule(Instant::now() + Duration::from_millis(50), 0);
        timer.schedule(Instant::now() + Duration::from_millis(100), 1);
        timer.cancel(token);

        assert_eq!(timer.wait().1, 1);
        assert!(timer.is_empty());
    }
}
