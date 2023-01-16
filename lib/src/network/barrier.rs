use super::message_dispatcher::{ChannelClosed, ContentSinkTrait, ContentStreamTrait};
use std::{fmt, mem::size_of};
use tokio::time::{self, Duration};

type BarrierId = u64;
type Round = u32;
#[derive(Eq, PartialEq, Copy, Clone)]
enum Step {
    Zero,
    One,
}
type Msg = (BarrierId, Round, Step);

/// Ensures there are no more in-flight messages beween us and the peer.
///
/// There are two aspects of this, first one is that we need to ignore all peer's message up until
/// they indicate to us that they are starting a new round of communication (this is necessary so
/// we can restart the crypto with a common state).
///
/// The second aspect is that we also need to cover the edge case when one peer restart its link
/// without the other one noticing. Then it could happen that the first peer sends a barrier
/// message to the second peer and that gets lost, the second peer (after the repo reload) will
/// then send a barrier message to the first while the first one will think it's the response to
/// it's first message. The second peer will then not receive its response.
///
/// The construction of the algorithm went as follows: the two peers need to ensure that the entire
/// barrier agreement happens within a single instance of Barrier on one side and a single instance
/// of a Barrier on the other side. To ensure this, both peers choose a random `barrier_id` which
/// they send to each other. This is then echoed from the other side and upon reception of the echo
/// each peer is able to check that the other side knows it's `barrier_id`. Let's call this process
/// "sync on barrier ID".
///
/// To be able to do the above, we needed to perform two steps:
///
/// Step #1: Send our barrier ID to the peer, and
/// Step #2: Receive our barrier ID from the peer.
///
/// As such, it may happen that one peer is currently performing the step #1 while the other peer
/// is performing the step #2. Thus we need to ensure that they're both performing the two steps in
/// sync. Let's call this process "sync on step".
///
/// Finally, because each "step" consists of sending and receiving (exchanging) a message, we must
/// ensure that the exchange does not happen across steps. Or in other words: it must not be the
/// case that a peer sends a message in one step, but receives a message from the other peer's
/// previous step. Let's call this process "sync on exchange".
///
/// TODO: This is one of those algorithms where a formal correctness proof would be welcome.
pub(super) struct Barrier<'a> {
    // Barrier ID is used to ensure that the other peer is communicating with this instance of
    // Barrier by sending us the ID back.
    barrier_id: BarrierId,
    stream: &'a mut (dyn ContentStreamTrait + Send + Sync + 'a),
    sink: &'a (dyn ContentSinkTrait + Send + Sync + 'a),
    #[cfg(test)]
    marker: Option<tests::StepMarker>,
}

impl<'a> Barrier<'a> {
    pub fn new<Stream, Sink>(stream: &'a mut Stream, sink: &'a Sink) -> Self
    where
        Stream: ContentStreamTrait + Send + Sync,
        Sink: ContentSinkTrait + Send + Sync,
    {
        Self {
            barrier_id: rand::random(),
            stream,
            sink,
            #[cfg(test)]
            marker: None,
        }
    }

    pub async fn run(&mut self) -> Result<(), BarrierError> {
        use std::cmp::max;

        #[cfg(test)]
        self.mark_step().await;

        #[cfg(test)]
        println!("{:x} >> RST", self.barrier_id);

        state_monitor!(barrier = "sending reset");
        // I think we send this empty message in order to break the encryption on the other side and
        // thus forcing it to start this barrier process again.
        self.sink.send(vec![]).await?;

        let mut next_round: u32 = 0;

        loop {
            let mut round = next_round;

            if round > 64 {
                tracing::error!("Barrier algorithm failed");
                return Err(BarrierError::Failure);
            }

            let (their_barrier_id, their_round, their_step) =
                self.exchange0(self.barrier_id, round).await?;

            if their_step != Step::Zero {
                next_round = max(round, their_round) + 1;
                continue;
            }

            // Just for info, this is ensured inside `exchange0`.
            assert!(round <= their_round);

            if round < their_round {
                state_monitor!(barrier = "catching up on step 0");
                // They are ahead of us, but on the same step. So play along, bump our round to
                // theirs and pretend we did the step zero with the same round.
                round = their_round;
                self.send(self.barrier_id, round, Step::Zero).await?;
            }

            let (our_barrier_id, their_round, their_step) =
                match self.exchange1(their_barrier_id, round).await? {
                    Some(msg) => msg,
                    None => {
                        next_round += 1;
                        continue;
                    }
                };

            if their_step != Step::One {
                next_round = max(round, their_round) + 1;
                continue;
            }

            if our_barrier_id != self.barrier_id {
                // Peer was communicating with our previous barrier, ignoring that.
                next_round = max(round, their_round) + 1;
                continue;
            }

            // Ensure we end at the same time.
            if round != their_round {
                next_round = max(round, their_round) + 1;
                continue;
            }

            break;
        }

        state_monitor!(barrier = "done");
        Ok(())
    }

    async fn exchange0(
        &mut self,
        barrier_id: BarrierId,
        our_round: Round,
    ) -> Result<Msg, BarrierError> {
        let our_step = Step::Zero;

        loop {
            state_monitor!(barrier = "step 0 sending");
            self.send(barrier_id, our_round, our_step).await?;

            loop {
                state_monitor!(barrier = "step 0 receiving");
                let (barrier_id, their_round, their_step) = match self
                    .recv(
                        #[cfg(test)]
                        our_round,
                        #[cfg(test)]
                        our_step,
                    )
                    .await?
                {
                    Some(msg) => msg,
                    None => continue,
                };

                if their_round < our_round {
                    // The peer is behind, so we resend our previous message. If they receive it
                    // more than once, they should ignore the duplicates. Note that we do need to
                    // resend it as opposed to just ignore and start receiving again because they
                    // could have dropped the previous message we sent them (e.g. because they did
                    // not have the repo that the two are about to sync).
                    break;
                }

                return Ok((barrier_id, their_round, their_step));
            }
        }
    }

    async fn exchange1(
        &mut self,
        barrier_id: BarrierId,
        our_round: Round,
    ) -> Result<Option<Msg>, BarrierError> {
        state_monitor!(barrier = "step 1 sending");
        let our_step = Step::One;
        self.send(barrier_id, our_round, our_step).await?;

        loop {
            state_monitor!(barrier = "step 1 receiving");
            let recv = self.recv(
                #[cfg(test)]
                our_round,
                #[cfg(test)]
                our_step,
            );

            // Timing out shouldn't be necessary, but it may still be useful if the peer is buggy.
            let result = match time::timeout(Duration::from_secs(5), recv).await {
                Ok(result) => result,
                Err(_) => {
                    // timeout
                    return Ok(None);
                }
            };

            match result? {
                Some((barrier, round, step)) => {
                    if step == Step::Zero && round == our_round {
                        // They resent the same message from previous step, ignore it.
                        continue;
                    }
                    return Ok(Some((barrier, round, step)));
                }
                None => return Ok(None),
            }
        }
    }

    async fn recv(
        &mut self,
        #[cfg(test)] round: Round,
        #[cfg(test)] our_step: Step,
    ) -> Result<Option<Msg>, BarrierError> {
        #[cfg(test)]
        self.mark_step().await;

        let msg = self.stream.recv().await?;

        match parse_message(&msg) {
            Some((barrier_id, their_round, their_step)) => {
                match their_step {
                    Step::Zero => {
                        #[cfg(test)]
                        println!(
                            "{:x} R{} S{:?} << their_barrier_id:{:x} their_round:{} their_step:{:?}",
                            self.barrier_id, round, our_step, barrier_id, their_round, their_step
                        )
                    }
                    Step::One => {
                        #[cfg(test)]
                        println!(
                            "{:x} R{} S{:?} << our_barrier_id:{:x} their_round:{} their_step:{:?}",
                            self.barrier_id, round, our_step, barrier_id, their_round, their_step
                        )
                    }
                }
                Ok(Some((barrier_id, their_round, their_step)))
            }
            // Ignore messages that belonged to whatever communication was going on prior us
            // starting this barrier process.
            None => Ok(None),
        }
    }

    async fn send(
        &mut self,
        barrier_id: BarrierId,
        our_round: Round,
        our_step: Step,
    ) -> Result<(), ChannelClosed> {
        #[cfg(test)]
        self.mark_step().await;

        #[cfg(test)]
        match our_step {
            Step::Zero => {
                assert_eq!(self.barrier_id, barrier_id);
                println!(
                    "{:x} R{} S0 >> self.barrier_id:{:x}",
                    self.barrier_id, our_round, barrier_id
                );
            }
            Step::One => {
                assert_ne!(self.barrier_id, barrier_id);
                println!(
                    "{:x} R{} S1 >> their_barrier_id:{:x}",
                    self.barrier_id, our_round, barrier_id
                );
            }
        }
        self.sink
            .send(construct_message(barrier_id, our_round, our_step).to_vec())
            .await
    }

    #[cfg(test)]
    async fn mark_step(&mut self) {
        if let Some(marker) = &mut self.marker {
            marker.mark_step().await
        }
    }
}

const MSG_STEP_SIZE: usize = size_of::<Step>();
const MSG_ID_SIZE: usize = size_of::<BarrierId>();
const MSG_ROUND_SIZE: usize = size_of::<Round>();
const MSG_PREFIX: &[u8; 13] = b"barrier-start";
const MSG_SUFFIX: &[u8; 11] = b"barrier-end";
const MSG_PREFIX_SIZE: usize = MSG_PREFIX.len();
const MSG_SUFFIX_SIZE: usize = MSG_SUFFIX.len();
const MSG_SIZE: usize =
    MSG_PREFIX_SIZE + MSG_ID_SIZE + MSG_ROUND_SIZE + MSG_STEP_SIZE + MSG_SUFFIX_SIZE;

type MsgData = [u8; MSG_SIZE];

fn construct_message(barrier_id: BarrierId, round: Round, step: Step) -> MsgData {
    let mut msg = [0u8; size_of::<MsgData>()];
    let s = &mut msg[..];

    s[..MSG_PREFIX_SIZE].clone_from_slice(MSG_PREFIX);
    let s = &mut s[MSG_PREFIX_SIZE..];

    s[..MSG_ID_SIZE].clone_from_slice(&barrier_id.to_le_bytes());
    let s = &mut s[MSG_ID_SIZE..];

    s[..MSG_ROUND_SIZE].clone_from_slice(&round.to_le_bytes());
    let s = &mut s[MSG_ROUND_SIZE..];

    match step {
        Step::Zero => s[..MSG_STEP_SIZE].clone_from_slice(&0u8.to_le_bytes()),
        Step::One => s[..MSG_STEP_SIZE].clone_from_slice(&1u8.to_le_bytes()),
    }
    let s = &mut s[MSG_STEP_SIZE..];

    s[..MSG_SUFFIX_SIZE].clone_from_slice(MSG_SUFFIX);

    msg
}

fn parse_message(data: &[u8]) -> Option<Msg> {
    if data.len() != MSG_SIZE {
        return None;
    }

    let (prefix, rest) = data.split_at(MSG_PREFIX_SIZE);

    if prefix != MSG_PREFIX {
        return None;
    }

    let (id_data, rest) = rest.split_at(MSG_ID_SIZE);
    let (round_data, rest) = rest.split_at(MSG_ROUND_SIZE);
    let (step_data, suffix) = rest.split_at(MSG_STEP_SIZE);

    if suffix != MSG_SUFFIX {
        return None;
    }

    let step_num = u8::from_le_bytes(step_data.try_into().unwrap());

    let step = match step_num {
        0 => Step::Zero,
        1 => Step::One,
        _ => return None,
    };

    // Unwraps OK because we know the sizes at compile time.
    Some((
        BarrierId::from_le_bytes(id_data.try_into().unwrap()),
        Round::from_le_bytes(round_data.try_into().unwrap()),
        step,
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum BarrierError {
    #[error("Barrier algorithm failed")]
    Failure,
    #[error("Channel closed")]
    ChannelClosed,
}

impl From<ChannelClosed> for BarrierError {
    fn from(_: ChannelClosed) -> Self {
        Self::ChannelClosed
    }
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => write!(f, "0"),
            Self::One => write!(f, "1"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoped_task::{self, ScopedJoinHandle};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::{
        sync::{mpsc, Mutex},
        task,
        time::{timeout, Duration},
    };

    struct Stepper {
        first: bool,
        pause_rx: mpsc::Receiver<()>,
        resume_tx: mpsc::Sender<()>,
        barrier_task: Option<ScopedJoinHandle<Result<(), BarrierError>>>,
    }

    impl Stepper {
        fn new(barrier_id: BarrierId, sink: Sink, mut stream: Stream) -> Stepper {
            let (pause_tx, pause_rx) = mpsc::channel(1);
            let (resume_tx, resume_rx) = mpsc::channel(1);

            let barrier_task = scoped_task::spawn(async move {
                Barrier {
                    barrier_id,
                    stream: &mut stream,
                    sink: &sink,
                    marker: Some(StepMarker {
                        pause_tx,
                        resume_rx,
                    }),
                }
                .run()
                .await
            });

            Self {
                first: true,
                pause_rx,
                resume_tx,
                barrier_task: Some(barrier_task),
            }
        }

        // When the `barrier_task` finishes, this returns `Some(result of the task)`, otherwise it
        // returns None.
        async fn step(&mut self) -> Option<Result<(), BarrierError>> {
            if !self.first {
                self.resume_tx.send(()).await.unwrap();
            }
            self.first = false;

            if self.pause_rx.recv().await.is_some() {
                None
            } else {
                let barrier_task = self.barrier_task.take();
                Some(barrier_task.unwrap().await.unwrap())
            }
        }

        async fn run_to_completion(&mut self) -> Result<(), BarrierError> {
            loop {
                if let Some(result) = self.step().await {
                    break result;
                }
            }
        }
    }

    pub(super) struct StepMarker {
        pause_tx: mpsc::Sender<()>,
        resume_rx: mpsc::Receiver<()>,
    }

    impl StepMarker {
        pub(super) async fn mark_step(&mut self) {
            self.pause_tx.send(()).await.unwrap();
            self.resume_rx.recv().await.unwrap();
        }
    }

    // --- Sink ---------------------------------------------------------------
    #[derive(Clone)]
    struct Sink {
        drop_count: Arc<Mutex<u32>>,
        tx: mpsc::Sender<Vec<u8>>,
    }

    #[async_trait]
    impl ContentSinkTrait for Sink {
        async fn send(&self, message: Vec<u8>) -> Result<(), ChannelClosed> {
            {
                let mut drop_count = self.drop_count.lock().await;
                if *drop_count > 0 {
                    *drop_count -= 1;
                    return Ok(());
                }
            }
            self.tx.send(message).await.map_err(|_| ChannelClosed)
        }
    }

    // --- Stream --------------------------------------------------------------
    #[derive(Clone)]
    struct Stream {
        rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    }

    #[async_trait]
    impl ContentStreamTrait for Stream {
        async fn recv(&mut self) -> Result<Vec<u8>, ChannelClosed> {
            let mut guard = self.rx.lock().await;
            let vec = guard.recv().await.unwrap();
            Ok(vec)
        }
    }

    // -------------------------------------------------------------------------
    fn new_test_channel(drop_count: u32) -> (Sink, Stream) {
        // Exchanging messages would normally require only a mpsc channel of size one, but at the
        // beginning of the Barrier algorithm we also send one "reset" message which increases the
        // channel size requirement by one.
        let (tx, rx) = mpsc::channel(2);
        (
            Sink {
                drop_count: Arc::new(Mutex::new(drop_count)),
                tx,
            },
            Stream {
                rx: Arc::new(Mutex::new(rx)),
            },
        )
    }
    // -------------------------------------------------------------------------

    #[derive(Debug)]
    enum Task1Result {
        CFinished,
        AFinished(Result<(), BarrierError>),
    }

    // When this returns true, it's no longer needed to test with higher `n`.
    async fn test_restart_case(n: u32) -> bool {
        println!(
            ">>>>>>>>>>>>>>>>>>> TEST RESTART AFTER n:{} <<<<<<<<<<<<<<<<<<<<<<",
            n
        );

        let (ac_to_b, b_from_ac) = new_test_channel(0 /* don't drop anything */);
        let (b_to_ac, ac_from_b) = new_test_channel(0 /* don't drop anything */);

        let task_1 = task::spawn(async move {
            let mut stepper_c = Stepper::new(0xc, ac_to_b.clone(), ac_from_b.clone());

            for _ in 0..n {
                if let Some(result) = stepper_c.step().await {
                    assert!(result.is_ok());
                    return Task1Result::CFinished;
                }
            }

            drop(stepper_c);

            let mut stepper_a = Stepper::new(0xa, ac_to_b, ac_from_b);
            Task1Result::AFinished(stepper_a.run_to_completion().await)
        });

        let task_2 = task::spawn(async move {
            let mut stepper = Stepper::new(0xb, b_to_ac, b_from_ac);
            stepper.run_to_completion().await.unwrap()
        });

        let task_c = task::spawn(async move {
            let r1 = task_1.await.unwrap();
            task_2.await.unwrap();

            match r1 {
                Task1Result::CFinished => (),
                Task1Result::AFinished(Ok(_)) => (),
                // This is a pathological case where 0xb finished while communicating with 0xc, but
                // 0xc has been interrupted right before it could finish. Then 0xa starts but 0xb
                // already moved on. I believe due to the CAP theorem there's nothing that can be
                // done in this case apart from 0xa restarting the process.
                Task1Result::AFinished(Err(BarrierError::ChannelClosed)) => (),
                result => panic!("Invalid result from task '0xa' {:?}", result),
            }

            matches!(r1, Task1Result::CFinished)
        });

        match timeout(Duration::from_secs(5), task_c).await {
            Err(_) => panic!("Test case n:{} timed out", n),
            Ok(Err(err)) => panic!("Test case n:{} failed with {:?}", n, err),
            Ok(Ok(is_done)) => is_done,
        }
    }

    #[tokio::test]
    async fn test_restarts() {
        let mut n = 0;
        loop {
            if test_restart_case(n).await {
                break;
            }
            n += 1;
        }
    }

    async fn test_drop_from_start_case(a: u32, b: u32) {
        println!(
            ">>>>>>>>>>>>>>>>>>> TEST DROP a:{} b:{} <<<<<<<<<<<<<<<<<<<<<<",
            a, b
        );

        let (a_to_b, mut b_from_a) = new_test_channel(a);
        let (b_to_a, mut a_from_b) = new_test_channel(b);

        let task_a = task::spawn(async move {
            Barrier {
                barrier_id: 0xa,
                stream: &mut a_from_b,
                sink: &a_to_b,
                marker: None,
            }
            .run()
            .await
            .unwrap()
        });

        let task_b = task::spawn(async move {
            Barrier {
                barrier_id: 0xb,
                stream: &mut b_from_a,
                sink: &b_to_a,
                marker: None,
            }
            .run()
            .await
            .unwrap()
        });

        match timeout(Duration::from_secs(5), task_a).await {
            Err(_) => panic!(
                "Test case drop_from_start (task_a, a:{}, b:{}) timed out",
                a, b
            ),
            Ok(Err(err)) => panic!(
                "Test case drop_from_start (task_a, a:{}, b:{}) failed with {:?}",
                a, b, err
            ),
            Ok(Ok(_)) => (),
        }

        match timeout(Duration::from_secs(5), task_b).await {
            Err(_) => panic!(
                "Test case drop_from_start (task_b, a:{}, b:{}) timed out",
                a, b
            ),
            Ok(Err(err)) => panic!(
                "Test case drop_from_start (task_b, a:{}, b:{}) failed with {:?}",
                a, b, err
            ),
            Ok(Ok(_)) => (),
        }
    }

    #[tokio::test]
    async fn test_drop_from_start() {
        // Drop first `a` packets from the barrier A and first `b` packets from the barrier B.
        // After sending first two packets, both nodes start reading, so max 2 dropped packets
        // make sense to consider.
        // Note that the case (2,2) should not be possible because that would mean that they both
        // sent a message while not receiving one.
        for (a, b) in [(0, 0), (1, 0), (1, 1), (2, 0), (2, 1)] {
            test_drop_from_start_case(a, b).await;
        }
    }
}
