use super::{connection::ConnectionDirection, peer_addr::PeerAddr, PeerSource, PublicRuntimeId};
use crate::crypto::sign::PublicKey;
use state_monitor::{MonitoredValue, StateMonitor};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{field, Span};

/// State monitor node for monitoring a network connection.
pub(super) struct ConnectionMonitor {
    span: Span,
    _source: MonitoredValue<PeerSource>,
    state: MonitoredValue<State>,
    permit_id: MonitoredValue<Option<u64>>,
    runtime_id: MonitoredValue<Option<PublicKey>>,
}

impl ConnectionMonitor {
    pub fn new(parent: &StateMonitor, addr: &PeerAddr, source: PeerSource) -> Self {
        let span = tracing::info_span!(
            "connection",
            %addr,
            ?source,
            runtime_id = field::Empty,
        );

        let direction_glyph = match ConnectionDirection::from_source(source) {
            ConnectionDirection::Incoming => '↓',
            ConnectionDirection::Outgoing => '↑',
        };

        // We need to ID the StateMonitor node because it is created prior to `addr` being
        // deduplicated and so we'd get an ambiguous entry otherwise.
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        let name = format!("id:{} {} {}", id, direction_glyph, addr);
        let node = parent.make_child(name);

        let source = node.make_value("source", source);
        let state = node.make_value("state", State::Idle);
        let permit_id = node.make_value("permit_id", None);
        let runtime_id = node.make_value("runtime_id", None);

        Self {
            span,
            _source: source,
            state,
            permit_id,
            runtime_id,
        }
    }

    pub fn span(&self) -> &Span {
        &self.span
    }

    pub fn start(&self) {
        *self.permit_id.get() = None;
        *self.runtime_id.get() = None;
        self.span.record("runtime_id", field::Empty);
    }

    pub fn mark_as_awaiting_permit(&self) {
        *self.state.get() = State::AwaitingPermit;
    }

    pub fn mark_as_connecting(&self, permit_id: u64) {
        *self.state.get() = State::Connecting;
        *self.permit_id.get() = Some(permit_id);
    }

    pub fn mark_as_handshaking(&self) {
        *self.state.get() = State::Handshaking;
    }

    pub fn mark_as_active(&self, runtime_id: PublicRuntimeId) {
        *self.state.get() = State::Active;
        *self.runtime_id.get() = Some(*runtime_id.as_public_key());
        self.span
            .record("runtime_id", field::debug(runtime_id.as_public_key()));
    }
}

#[derive(Debug)]
enum State {
    Idle,
    AwaitingPermit,
    Connecting,
    Handshaking,
    Active,
}
