use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    sync::watch,
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
        let on_change_tx = watch::channel(()).0;

        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                next_choker_id: AtomicUsize::new(0),
                on_change_tx,
                choked: Default::default(),
                unchoked: Default::default(),
            })),
        }
    }

    pub fn new_choker(&self) -> Choker {
        let mut inner = self.inner.lock().unwrap();

        let id = inner.next_choker_id.fetch_add(1, Ordering::Relaxed);

        if inner.unchoked.len() < MAX_UNCHOKED_COUNT {
            inner.unchoked.insert(id, UnchokedState::default());
        } else {
            inner.choked.insert(id, ChokedState::Uninterested);
        }

        Choker {
            inner: Arc::new(ChokerInner {
                manager_inner: self.inner.clone(),
                id,
            }),
            on_change_rx: inner.on_change_tx.subscribe(),
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
    on_change_tx: watch::Sender<()>,
    choked: HashMap<usize, ChokedState>,
    unchoked: HashMap<usize, UnchokedState>,
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

        // Unwrap OK because if `choker_id` is not in `unchoked`, it must be in `choked`.
        *self.choked.get_mut(&choker_id).unwrap() = ChokedState::Interested;

        // It's choked, check if we can unchoke something.
        if self.unchoked.len() < MAX_UNCHOKED_COUNT || self.try_evict_from_unchoked() {
            // Unwrap OK because we know `choked` is not empty (`choker_id` is in it).
            let to_unchoke = self.random_choked_and_interested().unwrap();

            assert!(self.choked.remove(&to_unchoke).is_some());
            self.unchoked.insert(to_unchoke, UnchokedState::default());

            if to_unchoke == choker_id {
                return GetPermitResult::Granted;
            } else {
                // TODO: Consider waking up only the one who just got unchoked.
                self.on_change_tx.send(()).unwrap_or(());
                // Unwrap OK because we know `unchoked` is not empty.
                let until = self.soonest_evictable().unwrap().1.evictable_at();
                return GetPermitResult::AwaitUntil(until);
            }
        }

        // Unwrap OK because we know `unchoked` is not empty.
        let until = self.soonest_evictable().unwrap().1.evictable_at();

        GetPermitResult::AwaitUntil(until)
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
        self.on_change_tx.send(()).unwrap_or(());
    }
}

#[derive(Clone)]
pub(crate) struct Choker {
    inner: Arc<ChokerInner>,
    on_change_rx: watch::Receiver<()>,
}

impl Choker {
    /// Halts forever when the `Manager` has already been destroyed.
    pub async fn wait_until_unchoked(&mut self) {
        loop {
            self.on_change_rx.borrow_and_update();

            let sleep_until = match self.get_permit() {
                GetPermitResult::Granted => return,
                GetPermitResult::AwaitUntil(sleep_until) => sleep_until,
            };

            match time::timeout_at(sleep_until, self.on_change_rx.changed()).await {
                Ok(Ok(())) | Err(_) => (),
                Ok(Err(_)) => unreachable!(),
            }
        }
    }

    fn get_permit(&mut self) -> GetPermitResult {
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
}

impl Drop for ChokerInner {
    fn drop(&mut self) {
        self.manager_inner.lock().unwrap().remove_choker(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use std::iter;

    // use simulated time (`start_paused`) to avoid having to wait for the timeout in the real time.
    #[tokio::test(start_paused = true)]
    async fn sanity() {
        let manager = Manager::new();
        let mut chokers: Vec<_> = iter::repeat_with(|| manager.new_choker())
            .take(MAX_UNCHOKED_COUNT + 1)
            .collect();

        for choker in chokers.iter_mut().take(MAX_UNCHOKED_COUNT) {
            assert_matches!(choker.get_permit(), GetPermitResult::Granted);
        }

        assert_matches!(
            chokers[MAX_UNCHOKED_COUNT].get_permit(),
            GetPermitResult::AwaitUntil(_)
        );

        tokio::time::timeout(
            PERMIT_INACTIVITY_TIMEOUT + Duration::from_millis(200),
            chokers[MAX_UNCHOKED_COUNT].wait_until_unchoked(),
        )
        .await
        .unwrap();
    }
}
