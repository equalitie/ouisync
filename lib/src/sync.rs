//! Synchronization utilities

/// MPMC broadcast channel
pub(crate) mod broadcast {
    pub use async_broadcast::Receiver;
    use tokio::task;

    /// Sender for a mpmc broadcast channel.
    ///
    /// This is a wrapper for `async_broadcast::Sender` which provides these additional features:
    ///
    /// 1. Receiver can be created directly from `Sender` (using `subscribe`), without needing to
    ///    keep a `Receiver` or `InactiveReceiver` around.
    /// 2. Calling `broadcast` when there are no subscribed receivers does not wait (block).
    #[derive(Clone)]
    pub struct Sender<T> {
        tx: async_broadcast::Sender<T>,
        rx: async_broadcast::InactiveReceiver<T>,
    }

    impl<T> Sender<T>
    where
        T: Clone + Send + 'static,
    {
        /// Create a broadcast channel with the specified capacity.
        pub fn new(capacity: usize) -> Self {
            let (tx, rx) = async_broadcast::broadcast(capacity);

            // HACK: drain the channel so that sending doesn't block when there are no
            // subscriptions.
            task::spawn({
                let mut rx = rx.clone();
                async move { while rx.recv().await.is_ok() {} }
            });

            Self {
                tx,
                rx: rx.deactivate(),
            }
        }

        /// Broadcast a message on the channel. The message will be received by all currently
        /// subscribed receivers. If the channel is full, then this function waits until it becomes
        /// non-full again. However, if there are currently no receivers, then this function
        /// returns immediately with `Ok`.
        pub async fn broadcast(&self, value: T) -> Result<(), async_broadcast::SendError<T>> {
            self.tx.broadcast(value).await?;
            Ok(())
        }

        /// Create a new receiver subscribed to this channel.
        pub fn subscribe(&self) -> async_broadcast::Receiver<T> {
            self.rx.activate_cloned()
        }

        /// Close the channel explicitly. This makes all subsequent receives to fail immediately.
        pub fn close(&self) {
            self.tx.close();
        }
    }
}
