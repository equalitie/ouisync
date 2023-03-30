use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) struct DebugRequest {}

impl DebugRequest {
    pub(crate) fn start() -> Self {
        Self {}
    }

    pub(crate) fn send(self) -> DebugRequestPayload {
        DebugRequestPayload {}
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
pub(crate) struct DebugRequestPayload {}

impl DebugRequestPayload {
    pub(crate) fn begin_reply(self) -> DebugResponse {
        DebugResponse {}
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Hash, Debug)]
pub(crate) struct DebugResponse {}

impl DebugResponse {
    pub(crate) fn send(self) -> DebugResponsePayload {
        DebugResponsePayload {}
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub(crate) struct DebugResponsePayload {}

impl DebugResponsePayload {
    pub(crate) fn unsolicited() -> Self {
        Self {}
    }

    pub(crate) fn received(self) -> DebugReceivedResponse {
        DebugReceivedResponse {}
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
pub(crate) struct DebugReceivedResponse {}
