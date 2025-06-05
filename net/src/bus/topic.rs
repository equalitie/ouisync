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

#[cfg(test)]
impl From<u128> for TopicNonce {
    fn from(n: u128) -> Self {
        Self(n.to_be_bytes())
    }
}

#[cfg(test)]
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
#[derive(Clone)]
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
            | (IncomingState::Failed, OutgoingState::Init) => {
                self.incoming = IncomingState::Creating;
                Some(Output::IncomingCreate)
            }
            (IncomingState::Failed, OutgoingState::Created) => {
                self.incoming = IncomingState::Creating;
                self.outgoing = OutgoingState::Init;
                Some(Output::IncomingCreate)
            }
            (IncomingState::Creating | IncomingState::Created(_), OutgoingState::Init) => {
                self.outgoing = OutgoingState::Creating;
                Some(Output::OutgoingCreate(self.nonce))
            }
            (IncomingState::Created(_), OutgoingState::Failed) => {
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
            (IncomingState::Creating, OutgoingState::Created)
            | (IncomingState::Created(_), OutgoingState::Creating)
            | (
                IncomingState::Creating | IncomingState::Failed,
                OutgoingState::Creating | OutgoingState::Failed,
            ) => {
                // Nothing to do, waiting for further inputs.
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
enum IncomingState {
    Init,
    Creating,
    Created(TopicNonce),
    Failed,
}

#[derive(Debug, Clone)]
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

    mod exhaustive {
        use super::super::*;
        use std::{
            collections::{BTreeSet, VecDeque},
            fmt,
        };

        // Simulate all possible executions of two peers establishing a connection on a single
        // topic.
        #[test]
        fn start() {
            // This test produces a lot of debug output, so remove all unnecessary formatting
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .compact()
                .with_target(false)
                .with_level(false)
                .without_time()
                .try_init()
                .ok();

            let client_nonce = TopicNonce::from(0u128);
            let server_nonce = TopicNonce::from(1u128);

            let client = Actor::new(ActorKind::Client, client_nonce);
            let server = Actor::new(ActorKind::Server, server_nonce);

            let mut actions = BTreeSet::new();

            actions.insert(Action::Poll(ActorKind::Client));
            actions.insert(Action::Poll(ActorKind::Server));

            // For debugging particular paths
            let path_to_take = VecDeque::new();

            // Derived experimentally, may be increased but better check why
            let max_depth = 13;

            run(client, server, actions, path_to_take, Vec::new(), max_depth);
        }

        // Recursively run actions in `actions` in all possible orders, adding new actions to the
        // set as needed. Recursion ends when there are no more actions to run.
        // Use `path_to_take` argument to debug a particular execution order.
        fn run(
            client: Actor,
            server: Actor,
            actions: BTreeSet<Action>,
            mut path_to_take: VecDeque<Action>,
            path_taken: Vec<Action>,
            max_depth: usize,
        ) {
            assert!(path_taken.len() <= max_depth);
            let pad = " ".repeat(path_taken.len() * 4);

            let predetermined_action = path_to_take.pop_front();

            let filtered_actions = if let Some(predetermined_action) = predetermined_action {
                // Pick action deterministically if we're debugging with `path_to_take`.
                let predetermined: BTreeSet<_> = actions
                    .iter()
                    .find(|action| **action == predetermined_action)
                    .iter()
                    .cloned()
                    .collect();

                assert!(
                    !predetermined.is_empty(),
                    "Action {predetermined_action:?} is not in {actions:?}"
                );

                predetermined
            } else {
                // Stop processing actions of actors which are done (have "accepted" a connection)
                actions
                    .iter()
                    .filter(|action| match action.actor_kind() {
                        ActorKind::Client => !client.is_done(),
                        ActorKind::Server => !server.is_done(),
                    })
                    .collect()
            };

            tracing::debug!("{pad} {path_taken:?}");
            tracing::debug!("{pad} {client:?}");
            tracing::debug!("{pad} {server:?}");

            // Check for termination
            if filtered_actions.is_empty() {
                tracing::debug!("{pad} No more work");

                assert!(
                    client.has_outgoing && server.has_incoming,
                    "Server did not establish outgoing streams"
                );
                assert!(
                    server.has_outgoing && client.has_incoming,
                    "Server did not establish outgoing streams"
                );
                assert!(
                    client.accepted_stream.unwrap() != server.accepted_stream.unwrap(),
                    "Actors did not agree on the same substream"
                );
                return;
            }

            for action in filtered_actions {
                let mut client = client.clone();
                let mut server = server.clone();

                let mut actions = actions.clone();

                assert!(actions.remove(&action));

                let (this_actor, that_actor) = match action.actor_kind() {
                    ActorKind::Client => (&mut client, &mut server),
                    ActorKind::Server => (&mut server, &mut client),
                };

                // Simulate the second half of the loop in `TopicHandler::run`.
                match action {
                    Action::Poll(_) => {
                        let tasks = this_actor.poll();

                        if tasks.create_incoming {
                            assert!(!this_actor.is_accepting);
                            this_actor.is_accepting = true;
                        }

                        if tasks.create_outgoing {
                            assert!(!this_actor.is_connecting);
                            this_actor.is_connecting = true;
                            assert!(actions.insert(Action::HandleAccept(that_actor.kind)));
                        }
                    }
                    Action::HandleAccept(_) => {
                        if this_actor.is_accepting {
                            assert!(!this_actor.has_incoming);

                            this_actor.is_accepting = false;
                            this_actor.has_incoming = true;

                            this_actor
                                .state
                                .handle(Input::IncomingCreated(that_actor.state.nonce));

                            actions.insert(Action::Poll(this_actor.kind));
                            actions.insert(Action::HandleConnect(that_actor.kind, true));
                        } else {
                            // Unsolicited accepts don't trigger `poll`.
                            actions.insert(Action::HandleConnect(that_actor.kind, false));
                        }
                    }
                    Action::HandleConnect(_, success) => {
                        assert!(this_actor.is_connecting);
                        assert!(!this_actor.has_outgoing);

                        this_actor.is_connecting = false;

                        if *success {
                            this_actor.has_outgoing = true;
                            this_actor.state.handle(Input::OutgoingCreated);
                        } else {
                            this_actor.state.handle(Input::OutgoingFailed);
                        }

                        actions.insert(Action::Poll(this_actor.kind));
                    }
                }

                let mut path_taken = path_taken.clone();
                path_taken.push(*action);

                run(
                    client,
                    server,
                    actions,
                    path_to_take.clone(),
                    path_taken,
                    max_depth,
                );
            }
        }

        #[derive(Debug, Clone, Copy, Eq, PartialEq)]
        enum StreamType {
            Incoming,
            Outgoing,
        }

        #[derive(Clone)]
        struct Actor {
            kind: ActorKind,
            state: TopicState,
            is_connecting: bool,
            is_accepting: bool,
            has_incoming: bool,
            has_outgoing: bool,
            accepted_stream: Option<StreamType>,
        }

        impl Actor {
            fn new(kind: ActorKind, topic_nonce: TopicNonce) -> Self {
                Self {
                    kind,
                    state: TopicState {
                        nonce: topic_nonce,
                        incoming: IncomingState::Init,
                        outgoing: OutgoingState::Init,
                    },
                    is_connecting: false,
                    is_accepting: false,
                    has_incoming: false,
                    has_outgoing: false,
                    accepted_stream: None,
                }
            }

            fn poll(&mut self) -> PollTasks {
                let mut tasks = PollTasks {
                    create_incoming: false,
                    create_outgoing: false,
                };

                // Simulate the first half of the loop in `TopicHandler::run`.
                while let Some(output) = self.state.poll() {
                    match output {
                        Output::IncomingCreate => {
                            tasks.create_incoming = true;
                        }
                        Output::IncomingAccept => {
                            assert!(self.has_incoming);
                            self.accepted_stream = Some(StreamType::Incoming);
                            break;
                        }
                        Output::OutgoingCreate(_) => {
                            tasks.create_outgoing = true;
                        }
                        Output::OutgoingAccept => {
                            assert!(self.has_outgoing);
                            self.accepted_stream = Some(StreamType::Outgoing);
                            break;
                        }
                    }
                }

                tasks
            }

            fn is_done(&self) -> bool {
                self.accepted_stream.is_some()
            }
        }

        impl fmt::Debug for Actor {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    "{:?} in:{:?} out:{:?}",
                    self.kind, self.state.incoming, self.state.outgoing
                )
            }
        }

        #[derive(Clone, Debug)]
        struct PollTasks {
            create_incoming: bool,
            create_outgoing: bool,
        }

        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
        enum ActorKind {
            Client,
            Server,
        }

        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
        enum Action {
            Poll(ActorKind),
            HandleAccept(ActorKind),
            HandleConnect(ActorKind, bool /* success */),
        }

        impl Action {
            fn actor_kind(&self) -> ActorKind {
                match self {
                    Self::Poll(actor) => *actor,
                    Self::HandleAccept(actor) => *actor,
                    Self::HandleConnect(actor, _) => *actor,
                }
            }
        }
    }
}
