use super::content_channel::{ContentSink, ContentStream};
use super::message_dispatcher::ChannelClosed;

type BarrierId = u64;
type Round = u32;
type Step = u8;
type Msg = (BarrierId, Round, Step);

use std::mem::size_of;

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
    stream: &'a mut ContentStream,
    sink: &'a mut ContentSink,
}

impl<'a> Barrier<'a> {
    pub fn new(stream: &'a mut ContentStream, sink: &'a mut ContentSink) -> Self {
        Self {
            barrier_id: rand::random(),
            stream,
            sink,
        }
    }

    pub async fn run(&mut self) -> Result<(), BarrierError> {
        use std::cmp::max;

        // I think we send this empty message in order to break the encryption on the other side and
        // thus forcing it to start this barrier process again.
        self.sink.send(vec![]).await?;

        let mut this_round: u32 = 0;

        loop {
            if this_round > 256 {
                tracing::error!("Barrier algorithm failed");
                return Err(BarrierError::Failure);
            }

            let (their_barrier_id, their_round, their_step) =
                self.exchange(self.barrier_id, this_round, 0).await?;

            if their_step != 0 {
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            // Ensure we end at the same time.
            if this_round != their_round {
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            let (our_barrier_id, their_round, their_step) =
                self.exchange(their_barrier_id, this_round, 1).await?;

            if their_step != 1 {
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            if our_barrier_id != self.barrier_id {
                // Peer was communicating with our previous barrier, ignoring that.
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            // Ensure we end at the same time.
            if this_round != their_round {
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            break;
        }

        Ok(())
    }

    async fn exchange(
        &mut self,
        barrier_id: BarrierId,
        our_round: Round,
        our_step: Step,
    ) -> Result<Msg, ChannelClosed> {
        self.sink
            .send(construct_message(barrier_id, our_round, our_step).to_vec())
            .await?;

        loop {
            let msg = self.stream.recv().await?;

            let (barrier_id, their_round, their_step) = match parse_message(&msg) {
                Some(msg) => msg,
                // Ignore messages that belonged to whatever communication was going on prior us
                // starting this barrier process.
                None => continue,
            };

            if their_step > our_step {
                // We have a miss-step in `exchange`, we just sent them that our step is 0 so
                // they'll see that and start a new round. Note that the next round will fail
                // because then they'll receive our message from step 1. But at least we'll have a
                // sync on `exchange` even though we'll not have a sync on "step". Sync on "step"
                // will then happen in the logic of the `run` function.
                continue;
            }

            return Ok((barrier_id, their_round, their_step));
        }
    }
}

fn construct_message(barrier_id: BarrierId, round: Round, step: Step) -> MsgData {
    let mut msg = [0u8; size_of::<MsgData>()];
    let s = &mut msg[..];

    s[0..MSG_PREFIX_SIZE].clone_from_slice(MSG_PREFIX);
    let s = &mut s[MSG_PREFIX_SIZE..];

    s[0..MSG_ID_SIZE].clone_from_slice(&barrier_id.to_le_bytes());
    let s = &mut s[MSG_ID_SIZE..];

    s[0..MSG_ROUND_SIZE].clone_from_slice(&round.to_le_bytes());
    let s = &mut s[MSG_ROUND_SIZE..];

    s[0..MSG_STEP_SIZE].clone_from_slice(&step.to_le_bytes());
    let s = &mut s[MSG_STEP_SIZE..];

    s[0..MSG_SUFFIX_SIZE].clone_from_slice(MSG_SUFFIX);

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

    // Unwraps OK because we know the sizes at compile time.
    Some((
        BarrierId::from_le_bytes(id_data.try_into().unwrap()),
        Round::from_le_bytes(round_data.try_into().unwrap()),
        Step::from_le_bytes(step_data.try_into().unwrap()),
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
