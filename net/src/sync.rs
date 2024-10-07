/// Single producer, single consumer, oneshot, rendezvous channel.
///
/// Unlike `tokio::sync::oneshot`, this one guarantees that the message is never lost even when the
/// receiver is dropped before receiving the message. Because of this, [`Sender::send`] must be
/// `async`.
///
/// # Cancel safety
///
/// If `send` is cancelled before completion, the value is still guaranteed to be received by the
/// receiver. If `recv` is cancelled before completion, the value is returned back from `send`. If
/// both `send` and `recv` are cancelled, the value is lost.
pub(crate) mod rendezvous {

    use std::{
        fmt,
        sync::{
            atomic::{AtomicU8, Ordering},
            Arc, Mutex,
        },
    };
    use tokio::sync::Notify;

    /// Sends a value to the associated [`Receiver`].
    pub struct Sender<T> {
        shared: Arc<Shared<T>>,
    }

    impl<T> Sender<T> {
        /// Sends the `value` to the [`Receiver`].
        pub async fn send(self, value: T) -> Result<(), T> {
            *self.shared.value.lock().unwrap() = Some(value);
            self.shared.tx_notify.notify_one();

            loop {
                self.shared.rx_notify.notified().await;

                let mut value = self.shared.value.lock().unwrap();

                if value.is_none() {
                    return Ok(());
                }

                if self.shared.state.get(RX_DROP) {
                    return Err(value.take().unwrap());
                }
            }
        }

        /// Waits for the associated [`Receiver`] to close (that is, to be dropped).
        pub async fn closed(&self) {
            loop {
                if self.shared.state.get(RX_DROP) {
                    break;
                }

                self.shared.rx_notify.notified().await;
            }
        }
    }

    impl<T> Drop for Sender<T> {
        fn drop(&mut self) {
            self.shared.state.set(TX_DROP);
            self.shared.tx_notify.notify_one();
        }
    }

    /// Receives a value from the associated [`Sender`].
    pub struct Receiver<T> {
        shared: Arc<Shared<T>>,
    }

    impl<T> Receiver<T> {
        /// Receives the value from the [`Sender`],
        ///
        /// If the sender is dropped before calling `send`, returns `RecvError`. Otherwise this is
        /// guaranteed to return the sent value.
        pub async fn recv(self) -> Result<T, RecvError> {
            loop {
                self.shared.tx_notify.notified().await;

                let value = self.shared.value.lock().unwrap().take();

                if let Some(value) = value {
                    self.shared.rx_notify.notify_one();
                    return Ok(value);
                }

                if self.shared.state.get(TX_DROP) {
                    return Err(RecvError);
                }
            }
        }
    }

    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            self.shared.state.set(RX_DROP);
            self.shared.rx_notify.notify_one();
        }
    }

    /// Create a rendezvous channel for sending a single message of type `T`.
    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let shared = Arc::new(Shared {
            value: Mutex::new(None),
            state: State::new(),
            tx_notify: Notify::new(),
            rx_notify: Notify::new(),
        });

        (
            Sender {
                shared: shared.clone(),
            },
            Receiver { shared },
        )
    }

    #[derive(Debug, Eq, PartialEq)]
    pub struct RecvError;

    impl fmt::Display for RecvError {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "channel closed")
        }
    }

    impl std::error::Error for RecvError {}

    struct Shared<T> {
        value: Mutex<Option<T>>,
        state: State,
        tx_notify: Notify,
        rx_notify: Notify,
    }

    struct State(AtomicU8);

    impl State {
        fn new() -> Self {
            Self(AtomicU8::new(0))
        }

        fn get(&self, flag: u8) -> bool {
            self.0.load(Ordering::Acquire) & flag == flag
        }

        fn set(&self, flag: u8) {
            self.0.fetch_or(flag, Ordering::AcqRel);
        }
    }

    const TX_DROP: u8 = 1;
    const RX_DROP: u8 = 2;

    #[cfg(test)]
    mod tests {
        use super::*;
        use futures_util::future;
        use tokio::task;

        #[tokio::test]
        async fn sanity_check() {
            let (tx, rx) = channel();

            let (tx_result, rx_result) = future::join(tx.send(1), rx.recv()).await;

            assert_eq!(tx_result, Ok(()));
            assert_eq!(rx_result, Ok(1));
        }

        #[tokio::test]
        async fn drop_tx_before_send() {
            let (tx, rx) = channel::<u32>();

            drop(tx);

            assert_eq!(rx.recv().await, Err(RecvError));
        }

        #[tokio::test]
        async fn drop_rx_before_send() {
            let (tx, rx) = channel::<u32>();

            drop(rx);

            assert_eq!(tx.send(1).await, Err(1));
        }

        #[tokio::test]
        async fn drop_rx_before_recv() {
            let (tx, rx) = channel::<u32>();

            let (tx_result, _) = future::join(tx.send(1), async move {
                task::yield_now().await;
                drop(rx)
            })
            .await;

            assert_eq!(tx_result, Err(1));
        }
    }
}
