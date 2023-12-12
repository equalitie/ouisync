use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    sync::Notify,
    time::{self, Duration, Instant},
};

const MAX_UNCHOKED_COUNT: usize = 3;
const PERMIT_DURATION_TIMEOUT: Duration = Duration::from_secs(30);
const PERMIT_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct Manager {
    inner: Arc<Mutex<ManagerInner>>,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                next_choker_id: AtomicUsize::new(0),
                choked: Default::default(),
                unchoked: Default::default(),
                notify: Arc::new(Notify::new()),
            })),
        }
    }

    pub fn new_choker(&self) -> Choker {
        let mut inner = self.inner.lock().unwrap();

        let id = inner.next_choker_id.fetch_add(1, Ordering::Relaxed);

        inner.choked.insert(id, ChokedState::Uninterested);

        Choker {
            inner: Arc::new(ChokerInner {
                manager_inner: self.inner.clone(),
                id,
                notify: inner.notify.clone(),
            }),
            choked: true,
        }
    }
}

#[derive(Eq, PartialEq)]
enum ChokedState {
    Interested,
    Uninterested,
}

#[derive(Clone, Copy)]
struct UnchokedState {
    unchoke_started: Instant,
    time_of_last_permit: Instant,
}

impl UnchokedState {
    fn is_evictable(&self) -> bool {
        Instant::now() >= self.evictable_at()
    }

    fn evictable_at(&self) -> Instant {
        let i1 = self.unchoke_started + PERMIT_DURATION_TIMEOUT;
        let i2 = self.time_of_last_permit + PERMIT_INACTIVITY_TIMEOUT;
        if i1 < i2 {
            i1
        } else {
            i2
        }
    }
}

impl Default for UnchokedState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            unchoke_started: now,
            time_of_last_permit: now,
        }
    }
}

struct ManagerInner {
    next_choker_id: AtomicUsize,
    choked: HashMap<usize, ChokedState>,
    unchoked: HashMap<usize, UnchokedState>,
    notify: Arc<Notify>,
}

#[derive(Debug)]
enum GetPermitResult {
    Granted,
    AwaitUntil(Instant),
}

impl ManagerInner {
    /// Does this:
    /// * If the `choker_id` is already unchoked it is granted a permit. Otherwise
    /// * if there is a free slot in `unchoked`, adds `choker_id` into it and grants it a permit.
    ///   Otherwise
    /// * check if some of the `unchoked` chokers can be evicted, if so evict them and
    ///   **some** choked choker takes its place. If the unchoked choker is `choker_id` then it
    ///   is granted a permit. Othewise
    /// * we calculate when the soonest unchoked choker is evictable and `choker_id` will
    ///   need to recheck at that time.
    fn get_permit(&mut self, choker_id: usize) -> GetPermitResult {
        if let Some(state) = self.unchoked.get_mut(&choker_id) {
            // It's unchoked, update permit and return.
            state.time_of_last_permit = Instant::now();
            return GetPermitResult::Granted;
        }

        self.choked.insert(choker_id, ChokedState::Interested);

        // It's choked, check if we can unchoke something.
        if self.unchoked.len() < MAX_UNCHOKED_COUNT || self.try_evict_from_unchoked() {
            // Unwrap OK because we know `choked` is not empty (`choker_id` is in it).
            let to_unchoke = self.random_choked_and_interested().unwrap();

            assert!(self.choked.remove(&to_unchoke).is_some());
            self.unchoked.insert(to_unchoke, UnchokedState::default());

            // Notify both the choked (if any) and the unchoked one.
            self.notify.notify_waiters();

            if to_unchoke == choker_id {
                GetPermitResult::Granted
            } else {
                // Unwrap OK because we know `unchoked` is not empty.
                let until = self.soonest_evictable().unwrap().1.evictable_at();
                GetPermitResult::AwaitUntil(until)
            }
        } else {
            // Unwrap OK because we know `unchoked` is not empty.
            let until = self.soonest_evictable().unwrap().1.evictable_at();
            GetPermitResult::AwaitUntil(until)
        }
    }

    // Return true if some choker was evicted from `unchoked` and inserted into `choked`.
    fn try_evict_from_unchoked(&mut self) -> bool {
        let to_evict = if let Some((id, state)) = self.soonest_evictable() {
            if state.is_evictable() {
                Some(id)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(to_evict) = to_evict {
            self.unchoked.remove(&to_evict);
            self.choked.insert(to_evict, ChokedState::Uninterested);
            true
        } else {
            false
        }
    }

    fn soonest_evictable(&self) -> Option<(usize, UnchokedState)> {
        let mut soonest: Option<(usize, UnchokedState)> = None;
        for (id, state) in &self.unchoked {
            let evictable_at = state.evictable_at();
            if let Some(old_soonest) = soonest {
                if evictable_at < old_soonest.1.evictable_at() {
                    soonest = Some((*id, *state));
                }
            } else {
                soonest = Some((*id, *state));
            }
        }
        soonest
    }

    fn random_choked_and_interested(&self) -> Option<usize> {
        use rand::Rng;

        let mut interested = self
            .choked
            .iter()
            .filter(|(_, state)| **state == ChokedState::Interested);

        let count = interested.clone().count();

        if count == 0 {
            return None;
        }

        interested
            .nth(rand::thread_rng().gen_range(0..count))
            .map(|(id, _)| *id)
    }

    fn remove_choker(&mut self, choker_id: usize) {
        self.choked.remove(&choker_id);
        self.unchoked.remove(&choker_id);
        self.notify.notify_waiters();
    }
}

#[derive(Clone)]
pub(crate) struct Choker {
    inner: Arc<ChokerInner>,
    choked: bool,
}

impl Choker {
    /// Waits until the state changes from choked to unchoked or from unchoked to choked. To find
    /// what state the choker is in currently, use `is_choked()`.
    pub async fn changed(&mut self) {
        loop {
            match (self.choked, self.get_permit()) {
                (true, GetPermitResult::Granted) => {
                    self.choked = false;
                    return;
                }
                (true, GetPermitResult::AwaitUntil(sleep_until)) => {
                    time::timeout_at(sleep_until, self.inner.notify.notified())
                        .await
                        .ok();
                }
                (false, GetPermitResult::Granted) => self.inner.notify.notified().await,
                (false, GetPermitResult::AwaitUntil(_)) => {
                    self.choked = true;
                    return;
                }
            };
        }
    }

    pub fn is_choked(&self) -> bool {
        self.choked
    }

    fn get_permit(&self) -> GetPermitResult {
        self.inner
            .manager_inner
            .lock()
            .unwrap()
            .get_permit(self.inner.id)
    }
}

struct ChokerInner {
    manager_inner: Arc<Mutex<ManagerInner>>,
    id: usize,
    notify: Arc<Notify>,
}

impl Drop for ChokerInner {
    fn drop(&mut self) {
        self.manager_inner.lock().unwrap().remove_choker(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::iter;

    // use simulated time (`start_paused`) to avoid having to wait for the timeout in the real time.
    #[tokio::test(start_paused = true)]
    async fn sanity() {
        let manager = Manager::new();

        let mut chokers: Vec<_> = iter::repeat_with(|| manager.new_choker())
            .take(MAX_UNCHOKED_COUNT + 1)
            .collect();

        // Initially everyone is choked
        for choker in &chokers {
            assert!(choker.is_choked());
        }

        // All but one get unchoked immediatelly.
        for choker in &mut chokers[..MAX_UNCHOKED_COUNT] {
            assert!(choker.changed().now_or_never().is_some());
            assert!(!choker.is_choked());
        }

        // One gets unchoked after the timeout.
        assert!(chokers[MAX_UNCHOKED_COUNT]
            .changed()
            .now_or_never()
            .is_none());

        chokers[MAX_UNCHOKED_COUNT].changed().await;
        assert!(!chokers[MAX_UNCHOKED_COUNT].is_choked());

        // And another one gets choked instead.
        let mut num_choked = 0;

        for choker in &mut chokers[..MAX_UNCHOKED_COUNT] {
            if choker.changed().now_or_never().is_some() {
                assert!(choker.is_choked());
                num_choked += 1;
            } else {
                assert!(!choker.is_choked());
            }
        }

        assert_eq!(num_choked, 1);
    }
}
