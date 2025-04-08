use rand::Rng;
use std::fmt;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct TopicId([u8; Self::SIZE]);

impl TopicId {
    pub const SIZE: usize = 32;

    pub fn generate<R: Rng>(rng: &mut R) -> Self {
        Self(rng.r#gen())
    }

    pub fn random() -> Self {
        Self::generate(&mut rand::thread_rng())
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl From<[u8; Self::SIZE]> for TopicId {
    fn from(array: [u8; Self::SIZE]) -> Self {
        Self(array)
    }
}

impl fmt::Debug for TopicId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:<8}", hex_fmt::HexFmt(&self.0))
    }
}

/// Random value to disambiguate incoming and outgoing streams bound to the same topic.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct TopicNonce([u8; Self::SIZE]);

impl TopicNonce {
    pub const SIZE: usize = 16;

    pub fn generate<R: Rng>(rng: &mut R) -> Self {
        Self(rng.r#gen())
    }

    pub fn random() -> Self {
        Self::generate(&mut rand::thread_rng())
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl From<[u8; Self::SIZE]> for TopicNonce {
    fn from(array: [u8; Self::SIZE]) -> Self {
        Self(array)
    }
}

impl From<u128> for TopicNonce {
    fn from(n: u128) -> Self {
        Self(n.to_be_bytes())
    }
}

impl From<u64> for TopicNonce {
    fn from(n: u64) -> Self {
        Self::from(u128::from(n))
    }
}

impl fmt::Debug for TopicNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:<8}", hex_fmt::HexFmt(&self.0))
    }
}

pub(super) enum Input {
    IncomingCreated(TopicNonce),
    IncomingFailed,
    OutgoingCreated,
    OutgoingFailed,
}

#[derive(Eq, PartialEq, Debug)]
pub(super) enum Output {
    OutgoingCreate(TopicNonce),
    OutgoingAccept,
    IncomingCreate,
    IncomingAccept,
}

/// State machine for establishing topic streams
pub(super) struct TopicState {
    pub nonce: TopicNonce,
    incoming: IncomingState,
    outgoing: OutgoingState,
}

impl TopicState {
    pub fn new(nonce: TopicNonce) -> Self {
        Self {
            nonce,
            incoming: IncomingState::Init,
            outgoing: OutgoingState::Init,
        }
    }

    pub fn handle(&mut self, input: Input) {
        match input {
            Input::IncomingCreated(nonce) => {
                self.incoming = IncomingState::Created(nonce);
            }
            Input::IncomingFailed => {
                self.incoming = IncomingState::Failed;
            }
            Input::OutgoingCreated => {
                self.outgoing = OutgoingState::Created;
            }
            Input::OutgoingFailed => {
                self.outgoing = OutgoingState::Failed;
            }
        }
    }

    pub fn poll(&mut self) -> Option<Output> {
        match (&self.incoming, &self.outgoing) {
            (IncomingState::Init, OutgoingState::Init)
            | (IncomingState::Init, OutgoingState::Creating)
            | (IncomingState::Init, OutgoingState::Created)
            | (IncomingState::Init, OutgoingState::Failed)
            | (IncomingState::Failed, OutgoingState::Created) => {
                // Create the incoming stream initially or try to create it again if it failed
                // previously but we just got the outgoing stream.
                self.incoming = IncomingState::Creating;
                Some(Output::IncomingCreate)
            }
            (IncomingState::Creating, OutgoingState::Init)
            | (IncomingState::Created(_), OutgoingState::Init)
            | (IncomingState::Created(_), OutgoingState::Failed)
            | (IncomingState::Failed, OutgoingState::Init) => {
                // Create the outgoing stream initially or try to create it again if it failed
                // previously but we got the incoming stream.
                self.outgoing = OutgoingState::Creating;
                Some(Output::OutgoingCreate(self.nonce))
            }
            (IncomingState::Created(incoming_nonce), OutgoingState::Created) => {
                // Both streams created, break the ties using the nonces.
                if *incoming_nonce > self.nonce {
                    Some(Output::IncomingAccept)
                } else {
                    Some(Output::OutgoingAccept)
                }
            }
            (IncomingState::Creating, OutgoingState::Creating)
            | (IncomingState::Creating, OutgoingState::Created)
            | (IncomingState::Creating, OutgoingState::Failed)
            | (IncomingState::Created(_), OutgoingState::Creating)
            | (IncomingState::Failed, OutgoingState::Creating)
            | (IncomingState::Failed, OutgoingState::Failed) => {
                // Nothing to do, waiting for further inputs.
                None
            }
        }
    }
}

enum IncomingState {
    Init,
    Creating,
    Created(TopicNonce),
    Failed,
}

enum OutgoingState {
    Init,
    Creating,
    Created,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanity_check() {
        for (this_nonce, that_nonce, expected_final_output) in [
            (
                TopicNonce::from(1u64),
                TopicNonce::from(2u64),
                Some(Output::IncomingAccept),
            ),
            (
                TopicNonce::from(2u64),
                TopicNonce::from(1u64),
                Some(Output::OutgoingAccept),
            ),
        ] {
            let mut state = TopicState::new(this_nonce);

            assert_eq!(state.poll(), Some(Output::IncomingCreate));
            assert_eq!(state.poll(), Some(Output::OutgoingCreate(this_nonce)));
            assert_eq!(state.poll(), None);

            state.handle(Input::IncomingCreated(that_nonce));

            assert_eq!(state.poll(), None);

            state.handle(Input::OutgoingCreated);

            assert_eq!(state.poll(), expected_final_output);
        }
    }

    #[test]
    fn outgoing_fail() {
        let this_nonce = TopicNonce::from(1u64);
        let that_nonce = TopicNonce::from(2u64);

        let mut state = TopicState::new(this_nonce);

        assert_eq!(state.poll(), Some(Output::IncomingCreate));
        assert_eq!(state.poll(), Some(Output::OutgoingCreate(this_nonce)));
        assert_eq!(state.poll(), None);

        state.handle(Input::OutgoingFailed);

        assert_eq!(state.poll(), None);

        state.handle(Input::IncomingCreated(that_nonce));

        assert_eq!(state.poll(), Some(Output::OutgoingCreate(this_nonce)));
        assert_eq!(state.poll(), None);

        state.handle(Input::OutgoingCreated);

        assert_eq!(state.poll(), Some(Output::IncomingAccept));
    }

    #[test]
    fn incoming_fail() {
        let this_nonce = TopicNonce::from(1u64);
        let that_nonce = TopicNonce::from(2u64);

        let mut state = TopicState::new(this_nonce);

        assert_eq!(state.poll(), Some(Output::IncomingCreate));
        assert_eq!(state.poll(), Some(Output::OutgoingCreate(this_nonce)));
        assert_eq!(state.poll(), None);

        state.handle(Input::IncomingFailed);

        assert_eq!(state.poll(), None);

        state.handle(Input::OutgoingCreated);

        assert_eq!(state.poll(), Some(Output::IncomingCreate));
        assert_eq!(state.poll(), None);

        state.handle(Input::IncomingCreated(that_nonce));

        assert_eq!(state.poll(), Some(Output::IncomingAccept));
    }
}
