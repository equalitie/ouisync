use super::{connection::ConnectionDirection, peer_addr::PeerAddr, PeerSource, PublicRuntimeId};
use crate::state_monitor::{MonitoredValue, StateMonitor};

/// State monitor node for monitoring a network connection.
pub(super) struct ConnectionMonitor {
    _source: MonitoredValue<PeerSource>,
    state: MonitoredValue<State>,
    permit_id: MonitoredValue<Option<u64>>,
    runtime_id: MonitoredValue<Option<PublicRuntimeId>>,
}

impl ConnectionMonitor {
    pub fn new(parent: &StateMonitor, addr: &PeerAddr, source: PeerSource) -> Self {
        let direction_glyph = match ConnectionDirection::from_source(source) {
            ConnectionDirection::Incoming => '↓',
            ConnectionDirection::Outgoing => '↑',
        };
        let name = format!("{} {}", direction_glyph, addr);
        let node = parent.make_child(name);

        let source = node.make_value("source", source);
        let state = node.make_value("state", State::Idle);
        let permit_id = node.make_value("permit_id", None);
        let runtime_id = node.make_value("runtime_id", None);

        Self {
            _source: source,
            state,
            permit_id,
            runtime_id,
        }
    }

    pub fn mark_as_awaiting_permit(&self) {
        *self.state.get() = State::AwaitingPermit;
        *self.permit_id.get() = None;
        *self.runtime_id.get() = None;
    }

    pub fn mark_as_connecting(&self, permit_id: u64) {
        *self.state.get() = State::Connecting;
        *self.permit_id.get() = Some(permit_id);
        *self.runtime_id.get() = None;
    }

    pub fn mark_as_handshaking(&self) {
        *self.state.get() = State::Handshaking;
        *self.runtime_id.get() = None;
    }

    pub fn mark_as_active(&self, runtime_id: PublicRuntimeId) {
        *self.state.get() = State::Active;
        *self.runtime_id.get() = Some(runtime_id);
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
