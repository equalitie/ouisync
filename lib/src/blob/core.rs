use crate::block::BLOCK_SIZE;
use std::sync::Arc;
use tokio::sync::Mutex;

// State shared among multiple instances of the same file.
#[derive(Debug)]
pub(crate) struct Core {
    pub len: u64,
}

impl Core {
    pub fn uninit() -> UninitCore {
        UninitCore(Self::new(0))
    }

    pub fn deep_clone(&self) -> Arc<Mutex<Self>> {
        Self::new(self.len)
    }

    fn new(len: u64) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self { len }))
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    pub fn block_count(&self) -> u32 {
        // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
        (1 + (self.len + super::HEADER_SIZE as u64 - 1) / BLOCK_SIZE as u64)
            .try_into()
            .unwrap_or(u32::MAX)
    }
}

pub(crate) struct UninitCore(Arc<Mutex<Core>>);

impl UninitCore {
    pub(super) fn init(self) -> Arc<Mutex<Core>> {
        self.0
    }

    // pub(crate)
}

pub(crate) struct MaybeInitCore {
    core: Arc<Mutex<Core>>,
    init: bool,
}

impl MaybeInitCore {
    pub(super) async fn ensure_init(self, len: u64) -> Arc<Mutex<Core>> {
        if !self.init {
            self.core.lock().await.len = len;
        }

        self.core
    }
}

impl From<Arc<Mutex<Core>>> for MaybeInitCore {
    fn from(core: Arc<Mutex<Core>>) -> Self {
        Self { core, init: true }
    }
}

impl From<UninitCore> for MaybeInitCore {
    fn from(core: UninitCore) -> Self {
        Self {
            core: core.0,
            init: false,
        }
    }
}
