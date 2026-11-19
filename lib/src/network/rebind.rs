//! Keeping the wanted stacks bound: rebinding the stacks which failed (e.g. when the network
//! interface went down while the app was backgrounded) or which couldn't be bound (e.g. because
//! their strict port was taken).

use super::{Inner, gateway::StackSlot};
use state_monitor::MonitoredValue;
use std::{collections::HashMap, sync::Arc};
use tokio::{
    select,
    time::{self, Duration, Instant},
};
use tracing::Instrument;

/// Max delay between two attempts to rebind the missing stacks.
const MAX_DELAY: Duration = Duration::from_secs(8);

/// How long the rebound stacks need to stay bound for the next failure to start a new series of
/// attempts (with no initial delay).
const STABLE_AFTER: Duration = Duration::from_secs(8);

impl Inner {
    /// Rebinds the stacks which are wanted but either not bound or failed, for as long as the
    /// network exists.
    pub(super) async fn keep_stacks_bound(self: Arc<Self>) {
        let monitor = self.main_monitor.make_child("Rebind");
        let mut monitored: HashMap<StackSlot, MonitoredValue<String>> = HashMap::new();
        let mut backoff = RebindBackoff::default();
        let mut wanted_rx = self.gateway.subscribe_wanted();

        loop {
            // Remove the stacks which are not missing anymore (rebound, or not wanted anymore)
            // from the monitor before waiting.
            let missing = self.gateway.missing_stacks();
            monitored.retain(|slot, _| missing.contains(slot));

            self.gateway.wait_for_missing_stack().await;

            // The wanted addresses changed (via `bind`), so start a new series of attempts.
            if wanted_rx.has_changed().unwrap_or(false) {
                wanted_rx.mark_unchanged();
                backoff.reset();
            }

            let missing = self.gateway.missing_stacks();
            let delay = backoff.next_delay(Instant::now());

            monitored.retain(|slot, _| missing.contains(slot));

            for slot in &missing {
                let value = monitored
                    .entry(*slot)
                    .or_insert_with(|| monitor.make_value(format!("{slot:?}"), String::new()));

                *value.get() = format!(
                    "rebinding (attempt {}, next in {:?})",
                    backoff.next_attempt(),
                    delay
                );
            }

            if !delay.is_zero() {
                tracing::debug!(parent: &self.span, ?missing, ?delay, "Next rebind attempt");

                select! {
                    _ = time::sleep(delay) => (),
                    result = wanted_rx.changed() => {
                        if result.is_err() {
                            return;
                        }

                        backoff.reset();
                        continue;
                    }
                }
            }

            let success = self.rebind_missing_stacks().await;
            backoff.record_attempt(success, Instant::now());
        }
    }

    /// Rebinds the stacks which are wanted but either not bound or failed. Returns whether all of
    /// them have been rebound.
    async fn rebind_missing_stacks(self: &Arc<Self>) -> bool {
        let _bind_guard = self.bind_lock.lock().await;

        // Check again, a `bind` could have changed things while we were waiting for the lock.
        let missing = self.gateway.missing_stacks();
        let stacks_id = self.gateway.stacks_id();
        let mut success = true;

        for slot in missing {
            if !self
                .gateway
                .rebind_stack(slot)
                .instrument(self.span.clone())
                .await
            {
                tracing::warn!(parent: &self.span, ?slot, "Failed to rebind stack, will retry");
                success = false;
            }
        }

        if self.gateway.stacks_id() != stacks_id {
            self.on_stacks_changed().await;
        }

        success
    }
}

/// Delays between the attempts to rebind the missing stacks: the first attempt is immediate, then
/// 1s, 2s, 4s and then 8s for each further attempt. A rebound stack which fails again shortly after
/// continues the series. The series starts over only after the stacks stayed bound for a while.
#[derive(Default)]
struct RebindBackoff {
    // Number of attempts in the current series.
    attempts: u32,
    // When the last attempt rebound all the stacks. `None` if it failed.
    rebound_at: Option<Instant>,
}

impl RebindBackoff {
    fn next_delay(&mut self, now: Instant) -> Duration {
        if self
            .rebound_at
            .is_some_and(|rebound_at| now.duration_since(rebound_at) >= STABLE_AFTER)
        {
            self.reset();
        }

        match self.attempts {
            0 => Duration::ZERO,
            attempts => Duration::from_secs(1 << (attempts - 1).min(3)).min(MAX_DELAY),
        }
    }

    /// Number of the next attempt in the current series (starting from 1).
    fn next_attempt(&self) -> u32 {
        self.attempts + 1
    }

    fn record_attempt(&mut self, success: bool, now: Instant) {
        self.attempts = self.attempts.saturating_add(1);
        self.rebound_at = success.then_some(now);
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Network, PeerAddr, PeerState};
    use state_monitor::{MonitorId, StateMonitor};
    use std::net::{Ipv4Addr, SocketAddr};

    #[test]
    fn backoff() {
        let mut backoff = RebindBackoff::default();
        let t0 = Instant::now();

        // A series of failed attempts.
        let mut delays = Vec::new();

        for _ in 0..6 {
            delays.push(backoff.next_delay(t0));
            backoff.record_attempt(false, t0);
        }

        assert_eq!(delays, [0, 1, 2, 4, 8, 8].map(Duration::from_secs).to_vec());

        // Success. A failure shortly after continues the series.
        backoff.record_attempt(true, t0);
        assert_eq!(
            backoff.next_delay(t0 + STABLE_AFTER / 2),
            Duration::from_secs(8)
        );

        // Success. A failure after the stacks have been stable for a while starts a new series.
        backoff.record_attempt(true, t0);
        assert_eq!(backoff.next_delay(t0 + STABLE_AFTER), Duration::ZERO);
        backoff.record_attempt(false, t0 + STABLE_AFTER);
        assert_eq!(
            backoff.next_delay(t0 + STABLE_AFTER),
            Duration::from_secs(1)
        );

        // Reset
        backoff.reset();
        assert_eq!(backoff.next_delay(t0), Duration::ZERO);
    }

    #[tokio::test]
    async fn failed_stack_is_rebound_on_same_port() {
        for strict in [false, true] {
            let network = Network::new();

            let port = if strict { free_udp_port() } else { 0 };
            network
                .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, port).into())])
                .await;

            let port = bound_quic_port(&network).unwrap();
            network.inner.gateway.kill_quic_stack(StackSlot::QuicV4);

            wait_until(|| {
                network.inner.gateway.missing_stacks().is_empty()
                    && bound_quic_port(&network) == Some(port)
            })
            .await;
        }
    }

    #[tokio::test]
    async fn taken_strict_port_is_retried() {
        let monitor = StateMonitor::make_root();
        let network = Network::builder().monitor(monitor.clone()).build();

        let taken = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = taken.local_addr().unwrap().port();

        network
            .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, port).into())])
            .await;
        assert_eq!(bound_quic_port(&network), None);

        // The retries are shown in the monitor.
        wait_until(|| rebind_status(&monitor).is_some()).await;

        drop(taken);

        wait_until(|| bound_quic_port(&network) == Some(port)).await;
        wait_until(|| rebind_status(&monitor).is_none()).await;
    }

    #[tokio::test]
    async fn bind_cancels_retries() {
        let monitor = StateMonitor::make_root();
        let network = Network::builder().monitor(monitor.clone()).build();

        let taken = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken_port = taken.local_addr().unwrap().port();

        network
            .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, taken_port).into())])
            .await;
        wait_until(|| rebind_status(&monitor).is_some()).await;

        // Bind to a random port instead. Nothing is missing anymore.
        network
            .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;
        let port = bound_quic_port(&network).unwrap();
        assert_ne!(port, taken_port);

        wait_until(|| rebind_status(&monitor).is_none()).await;

        // The previous strict port is not retried anymore, even when it becomes free.
        drop(taken);
        time::sleep(Duration::from_secs(2)).await;
        assert_eq!(bound_quic_port(&network), Some(port));
    }

    #[tokio::test]
    async fn peers_reconnect_after_failure() {
        let a = Network::new();
        a.bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;

        let b = Network::new();
        b.bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;
        let b_addr = PeerAddr::Quic(SocketAddr::from((
            Ipv4Addr::LOCALHOST,
            bound_quic_port(&b).unwrap(),
        )));

        a.add_user_provided_peer(&b_addr);
        wait_until(|| is_active(&a, b_addr)).await;

        a.inner.gateway.kill_quic_stack(StackSlot::QuicV4);
        wait_until(|| !is_active(&a, b_addr)).await;
        wait_until(|| is_active(&a, b_addr)).await;
    }

    fn free_udp_port() -> u16 {
        std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn bound_quic_port(network: &Network) -> Option<u16> {
        network
            .listener_local_addrs()
            .into_iter()
            .find(|addr| addr.is_quic())
            .map(|addr| addr.port())
    }

    fn rebind_status(monitor: &StateMonitor) -> Option<String> {
        monitor
            .locate([MonitorId::new("Rebind".to_owned(), 0)])?
            .get_value("QuicV4")
            .ok()
    }

    fn is_active(network: &Network, addr: PeerAddr) -> bool {
        network
            .peer_info(addr)
            .is_some_and(|info| matches!(info.state, PeerState::Active { .. }))
    }

    async fn wait_until(mut condition: impl FnMut() -> bool) {
        time::timeout(Duration::from_secs(30), async {
            while !condition() {
                time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }
}
