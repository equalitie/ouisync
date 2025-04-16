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

#[derive(Debug)]
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
            (
                IncomingState::Init,
                OutgoingState::Init
                | OutgoingState::Creating
                | OutgoingState::Created
                | OutgoingState::Failed,
            )
            | (IncomingState::Failed, OutgoingState::Init | OutgoingState::Creating) => {
                self.incoming = IncomingState::Creating;
                Some(Output::IncomingCreate)
            }
            (IncomingState::Failed, OutgoingState::Created) => {
                self.incoming = IncomingState::Creating;
                self.outgoing = OutgoingState::Init;
                Some(Output::IncomingCreate)
            }
            (IncomingState::Creating | IncomingState::Created(_), OutgoingState::Init)
            | (IncomingState::Creating, OutgoingState::Failed) => {
                self.outgoing = OutgoingState::Creating;
                Some(Output::OutgoingCreate(self.nonce))
            }
            (IncomingState::Created(_), OutgoingState::Failed) => {
                self.incoming = IncomingState::Init;
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
            | (IncomingState::Created(_), OutgoingState::Creating)
            | (IncomingState::Failed, OutgoingState::Failed) => {
                // Nothing to do, waiting for further inputs.
                None
            }
        }
    }
}

#[derive(Debug)]
enum IncomingState {
    Init,
    Creating,
    Created(TopicNonce),
    Failed,
}

#[derive(Debug)]
enum OutgoingState {
    Init,
    Creating,
    Created,
    Failed,
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::*;
    use proptest::{
        arbitrary::{any, Arbitrary},
        collection::vec,
        num,
        prelude::Strategy,
        strategy::{Just, Map, Union},
    };
    use test_strategy::proptest;

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

    #[proptest]
    fn proptest(
        this_nonce: TopicNonce,
        that_nonce: TopicNonce,
        #[strategy(vec(any::<Response>(), 0..20))] incoming_responses: Vec<Response>,
        #[strategy(vec(any::<Response>(), 0..20))] outgoing_responses: Vec<Response>,
    ) {
        crate::test_utils::init_log();

        proptest_case(
            this_nonce,
            that_nonce,
            incoming_responses,
            outgoing_responses,
        );
    }

    #[test]
    fn debug() {
        crate::test_utils::init_log();

        proptest_case(
            TopicNonce::from(0u128),
            TopicNonce::from(0u128),
            vec![],
            vec![Response::Pending],
        );
    }

    fn proptest_case(
        this_nonce: TopicNonce,
        that_nonce: TopicNonce,
        incoming_responses: Vec<Response>,
        outgoing_responses: Vec<Response>,
    ) {
        let mut incoming_responses = incoming_responses
            .into_iter()
            .chain(iter::repeat(Response::Success));

        let mut outgoing_responses = outgoing_responses
            .into_iter()
            .chain(iter::repeat(Response::Success));

        let mut state = TopicState::new(this_nonce);

        // Simulate tasks that create the respective streams.
        let mut incoming_task = TaskState::Waiting;
        let mut outgoing_task = TaskState::Waiting;

        let max_iters = 1000; // prevent running forever
        let mut iters = 0;

        loop {
            let mut progress = false;

            while let Some(output) = state.poll() {
                progress = true;

                match output {
                    Output::IncomingCreate => {
                        incoming_task = TaskState::Running;
                    }
                    Output::IncomingAccept => {
                        assert_eq!(incoming_task, TaskState::Success);
                        tracing::debug!("success (incoming)");
                        return;
                    }
                    Output::OutgoingCreate(_) => {
                        outgoing_task = TaskState::Running;
                    }
                    Output::OutgoingAccept => {
                        assert_eq!(outgoing_task, TaskState::Success);
                        tracing::debug!("success (outgoing)");
                        return;
                    }
                }
            }

            if let TaskState::Running = incoming_task {
                match incoming_responses.next().unwrap() {
                    Response::Pending => (),
                    Response::Success => {
                        incoming_task = TaskState::Success;
                        state.handle(Input::IncomingCreated(that_nonce));
                    }
                    Response::Failure => {
                        incoming_task = TaskState::Failure;
                        state.handle(Input::IncomingFailed);
                    }
                }

                progress = true;
            }

            if let TaskState::Running = outgoing_task {
                match outgoing_responses.next().unwrap() {
                    Response::Pending => (),
                    Response::Success => {
                        outgoing_task = TaskState::Success;
                        state.handle(Input::OutgoingCreated);
                    }
                    Response::Failure => {
                        outgoing_task = TaskState::Failure;
                        state.handle(Input::OutgoingFailed);
                    }
                }

                progress = true;
            }

            iters += 1;

            if iters >= max_iters {
                panic!("iteration limit reached");
            }

            if !progress {
                match (incoming_task, outgoing_task) {
                    (TaskState::Failure, TaskState::Failure) => {
                        // This is not considered a test failure
                        tracing::debug!("both streams failed");
                        return;
                    }
                    _ => {
                        panic!("no progress made (incoming = {incoming_task:?}, outgoing = {outgoing_task:?})");
                    }
                }
            }
        }
    }

    #[derive(Clone, Copy, Eq, PartialEq, Debug)]
    enum TaskState {
        Waiting,
        Running,
        Success,
        Failure,
    }

    #[derive(Clone, Copy, Debug)]
    enum Response {
        Pending,
        Success,
        Failure,
    }

    impl Arbitrary for Response {
        type Parameters = ();
        type Strategy = Union<Just<Self>>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            Union::new([
                Just(Self::Pending),
                Just(Self::Success),
                Just(Self::Failure),
            ])
        }
    }

    impl Arbitrary for TopicNonce {
        type Parameters = ();
        type Strategy = Map<num::u128::Any, fn(u128) -> Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            any::<u128>().prop_map(Self::from)
        }
    }
}
