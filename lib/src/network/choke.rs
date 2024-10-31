use std::{fmt, sync::Arc};

use tokio::{
    sync::{Semaphore, SemaphorePermit, TryAcquireError},
    time::{self, Instant},
};

use super::constants::{MAX_UNCHOKED_COUNT, MAX_UNCHOKED_DURATION};

/// Mechanism to ensure only a given number of peers are active (sending/receiving messages from us)
/// at any given time but rotates them in regular intervals so that every peer gets equal time share.
#[derive(Clone)]
pub(super) struct Choker {
    shared: Arc<Semaphore>,
}

impl Choker {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Semaphore::new(MAX_UNCHOKED_COUNT)),
        }
    }

    pub fn choke(&self) -> Choked<'_> {
        Choked {
            shared: &self.shared,
        }
    }
}

pub(super) struct Choked<'a> {
    shared: &'a Semaphore,
}

impl<'a> Choked<'a> {
    /// Waits until unchoked.
    pub async fn unchoke(self) -> Unchoked<'a> {
        // unwrap is OK because we never close the semaphore.
        let permit = self.shared.acquire().await.unwrap();
        let expiry = Instant::now() + MAX_UNCHOKED_DURATION;

        Unchoked {
            shared: self.shared,
            permit,
            expiry,
        }
    }

    pub fn try_unchoke(self) -> Result<Unchoked<'a>, Self> {
        match self.shared.try_acquire() {
            Ok(permit) => Ok(Unchoked {
                shared: self.shared,
                permit,
                expiry: Instant::now() + MAX_UNCHOKED_DURATION,
            }),
            Err(TryAcquireError::NoPermits) => Err(Self {
                shared: self.shared,
            }),
            Err(TryAcquireError::Closed) => unreachable!(),
        }
    }
}

impl fmt::Debug for Choked<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Choked").finish_non_exhaustive()
    }
}

pub(super) struct Unchoked<'a> {
    shared: &'a Semaphore,
    #[allow(dead_code)]
    permit: SemaphorePermit<'a>,
    expiry: Instant,
}

impl<'a> Unchoked<'a> {
    /// Waits until choked.
    pub async fn choke(self) -> Choked<'a> {
        time::sleep_until(self.expiry).await;

        Choked {
            shared: self.shared,
        }
    }
}

impl fmt::Debug for Unchoked<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unchoked")
            .field("expiry", &self.expiry)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn under_unchoke_count_limit() {
        let choker = Choker::new();
        let choked = choker.choke();

        // The client is unchoked immediately
        let mut unchoked = choked.try_unchoke().unwrap();

        for _ in 0..10 {
            // There is only one client (which is less than `MAX_UNCHOKED_COUNT`), so `try_unchoke`
            // always succeeds.
            unchoked = unchoked.choke().await.try_unchoke().unwrap();
        }
    }

    // TODO:
    // #[tokio::test(start_paused = true)]
    // async fn over_unchoke_count_limit() {
    //     let choker = Choker::new();
    // }
}
