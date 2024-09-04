pub(crate) mod rendezvous {
    //! Single producer, single consumer, oneshot, rendezvous channel.
    //!
    //! Unlike `tokio::sync::oneshot`, this one guarantees that the message is never lost even when
    //! the receiver is dropped before receiving the message. Because of this,[`Sender::send`] must
    //! be `async`.
    //!
    //! # Cancel safety
    //!
    //! If `send` is cancelled before completion, the value is still guaranteed to be received by
    //! the receiver. If `recv` is cancelled before completion, the value is returned back from
    //! `send`. If both `send` and `recv` are cancelled, the value is lost.

    use std::{
        fmt,
        sync::{Arc, Mutex},
    };
    use tokio::sync::Notify;

    /// Sends a value to the associated [`Receiver`].
    pub struct Sender<T> {
        shared: Arc<Shared<T>>,
    }

    impl<T> Sender<T> {
        /// Sends the `value` to the [`Receiver`].
        pub async fn send(self, value: T) -> Result<(), T> {
            self.shared.state.lock().unwrap().value = Some(value);
            self.shared.tx_notify.notify_one();

            loop {
                self.shared.rx_notify.notified().await;

                let mut state = self.shared.state.lock().unwrap();

                if state.value.is_none() {
                    return Ok(());
                }

                if state.rx_drop {
                    return Err(state.value.take().unwrap());
                }
            }
        }
    }

    impl<T> Drop for Sender<T> {
        fn drop(&mut self) {
            self.shared.state.lock().unwrap().tx_drop = true;
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

                let mut state = self.shared.state.lock().unwrap();

                if let Some(value) = state.value.take() {
                    self.shared.rx_notify.notify_one();
                    return Ok(value);
                }

                if state.tx_drop {
                    return Err(RecvError);
                }
            }
        }
    }

    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            self.shared.state.lock().unwrap().rx_drop = true;
            self.shared.rx_notify.notify_one();
        }
    }

    /// Create a rendezvous channel for sending a single message of type `T`.
    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                value: None,
                tx_drop: false,
                rx_drop: false,
            }),
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
        state: Mutex<State<T>>,
        tx_notify: Notify,
        rx_notify: Notify,
    }

    struct State<T> {
        value: Option<T>,
        tx_drop: bool,
        rx_drop: bool,
    }

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
