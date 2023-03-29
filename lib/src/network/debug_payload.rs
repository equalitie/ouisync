use serde::{Deserialize, Serialize};
use std::{fmt, time::Instant};

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) struct DebugRequest {
    created: Instant,
}

impl DebugRequest {
    pub(crate) fn start() -> Self {
        Self {
            created: Instant::now(),
        }
    }

    pub(crate) fn send(self) -> DebugRequestPayload {
        DebugRequestPayload {
            created: self.created,
            sent: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
pub(crate) struct DebugRequestPayload {
    #[serde(with = "serde_millis")]
    created: Instant,
    #[serde(with = "serde_millis")]
    sent: Instant,
}

impl DebugRequestPayload {
    pub(crate) fn begin_reply(self) -> DebugResponse {
        DebugResponse {
            request_created: self.created,
            request_sent: self.sent,
            request_received: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Hash, Debug)]
pub(crate) struct DebugResponse {
    #[serde(with = "serde_millis")]
    request_created: Instant,
    #[serde(with = "serde_millis")]
    request_sent: Instant,
    #[serde(with = "serde_millis")]
    request_received: Instant,
}

impl DebugResponse {
    pub(crate) fn send(self) -> DebugResponsePayload {
        DebugResponsePayload {
            request: Some(self),
            response_sent: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub(crate) struct DebugResponsePayload {
    request: Option<DebugResponse>,
    #[serde(with = "serde_millis")]
    response_sent: Instant,
}

impl DebugResponsePayload {
    pub(crate) fn unsolicited() -> Self {
        Self {
            request: None,
            response_sent: Instant::now(),
        }
    }

    pub(crate) fn received(self) -> DebugReceivedResponse {
        DebugReceivedResponse {
            request: self.request,
            response_sent: self.response_sent,
            response_enqueued: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub(crate) struct DebugReceivedResponse {
    request: Option<DebugResponse>,
    #[serde(with = "serde_millis")]
    response_sent: Instant,
    #[serde(with = "serde_millis")]
    response_enqueued: Instant,
}

impl fmt::Debug for DebugReceivedResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let now = Instant::now();
        if let Some(_request) = self.request {
            write!(
                f,
                "DebugReceivedResponse(sent-enqueued:{:?}, enqueued-handled:{:?})",
                self.response_enqueued - self.response_enqueued,
                now - self.response_enqueued,
            )
        } else {
            write!(f, "DebugReceivedResponse(unsolicited)",)
        }
    }
}
