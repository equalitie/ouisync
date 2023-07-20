use crate::{blob::BlobId, collections::HashMap, deadlock::BlockingMutex};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, SemaphorePermit};

// Maximum number of `File::progress` calls that can be executed concurrently. We limit this because
// the operation is currently slow and we don't want to exhaust all db connections when multiple
// files are queried for progress.
const MAX_CONCURRENT_QUERIES: usize = 2;

// Cache entries expire after this time.
const EXPIRY: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct FileProgressCache {
    shared: Arc<Shared>,
}

impl FileProgressCache {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                semaphore: Semaphore::new(MAX_CONCURRENT_QUERIES),
                block_counts: BlockingMutex::new(HashMap::default()),
            }),
        }
    }

    pub async fn acquire(&self) -> Permit<'_> {
        // unwrap is ok because we never `close` the semaphore.
        let permit = self.shared.semaphore.acquire().await.unwrap();

        Permit {
            shared: &self.shared,
            permit,
        }
    }

    pub fn reset(&self, blob_id: &BlobId, new_count: u32) {
        if let Some((count, _)) = self.shared.block_counts.lock().unwrap().get_mut(blob_id) {
            *count = new_count;
        }
    }
}

struct Shared {
    semaphore: Semaphore,
    block_counts: BlockingMutex<HashMap<BlobId, (u32, Instant)>>,
}

pub(crate) struct Permit<'a> {
    shared: &'a Shared,
    permit: SemaphorePermit<'a>,
}

impl<'a> Permit<'a> {
    pub fn get(self, blob_id: BlobId) -> Entry<'a> {
        let mut block_counts = self.shared.block_counts.lock().unwrap();

        block_counts.retain(|_, (_, timestamp)| timestamp.elapsed() < EXPIRY);

        let value = block_counts
            .get(&blob_id)
            .map(|(value, _)| *value)
            .unwrap_or(0);

        Entry {
            shared: self.shared,
            blob_id,
            value,
            _permit: self.permit,
        }
    }
}

pub(crate) struct Entry<'a> {
    shared: &'a Shared,
    blob_id: BlobId,
    value: u32,
    _permit: SemaphorePermit<'a>,
}

impl Deref for Entry<'_> {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl DerefMut for Entry<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl Drop for Entry<'_> {
    fn drop(&mut self) {
        self.shared
            .block_counts
            .lock()
            .unwrap()
            .entry(self.blob_id)
            .and_modify(|(value, timestamp)| {
                if self.value > *value {
                    *value = self.value;
                    *timestamp = Instant::now();
                }
            })
            .or_insert_with(|| (0, Instant::now()));
    }
}
