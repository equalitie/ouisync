use super::content_channel::{ContentSink, ContentStream};
use super::message_dispatcher::ChannelClosed;

type BarrierId = u64;
type Round = u32;
type Msg = (MsgType, Round, BarrierId);

use std::mem::size_of;

enum Step { One, Two }
enum MsgType { Step1, Step2, Reset }

const MSG_TYPE_SIZE: usize = size_of::<u8>();
const MSG_ID_SIZE: usize = size_of::<BarrierId>();
const MSG_ROUND_SIZE: usize = size_of::<Round>();
const MSG_PREFIX: &[u8; 13] = b"barrier-start";
const MSG_SUFFIX: &[u8; 11] = b"barrier-end";
const MSG_PREFIX_SIZE: usize = MSG_PREFIX.len();
const MSG_SUFFIX_SIZE: usize = MSG_SUFFIX.len();
const MSG_SIZE: usize =
    MSG_PREFIX_SIZE + MSG_ID_SIZE + MSG_ROUND_SIZE + MSG_TYPE_SIZE + MSG_SUFFIX_SIZE;

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

            let (msg_type, their_round, their_barrier_id) =
                self.exchange(Step::One, this_round, self.barrier_id).await?;

            // Ensure we end at the same time.
            if this_round != their_round {
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            match msg_type {
                MsgType::Reset => {
                    continue;
                }
                MsgType::Step1 => (),
                MsgType::Step2 => {
                    self.send_reset(this_round).await?;
                    continue;
                }
            }

            let (msg_type, their_round, our_barrier_id) =
                self.exchange(Step::Two, this_round, their_barrier_id).await?;

            // Ensure we end at the same time.
            if this_round != their_round {
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            match msg_type {
                MsgType::Reset => {
                    this_round = max(this_round + 1, their_round);
                    continue;
                }
                MsgType::Step1 => {
                    this_round = max(this_round, their_round) + 1;
                    self.send_reset(this_round).await?;
                    continue;
                }
                MsgType::Step2 => (),
            }

            if our_barrier_id != self.barrier_id {
                // Peer was communicating with our previous barrier, ignoring that.
                this_round = max(this_round, their_round) + 1;
                continue;
            }

            break;
        }

        Ok(())
    }

    async fn exchange(
        &mut self,
        step: Step,
        our_round: Round,
        barrier_id: BarrierId,
    ) -> Result<Msg, ChannelClosed> {
        let msg_type = match step {
            Step::One => MsgType::Step1,
            Step::Two => MsgType::Step2,
        };

        self.sink
            .send(construct_message(msg_type, our_round, barrier_id).to_vec())
            .await?;

        loop {
            if let Some(msg) = parse_message(&self.stream.recv().await?) {
                return Ok(msg);
            }
        }
    }

    async fn send_reset(&mut self, round: Round) -> Result<(), ChannelClosed> {
        self.sink
            .send(construct_message(MsgType::Reset, round, 0).to_vec())
            .await
    }
}

fn construct_message(msg_type: MsgType, round: Round, barrier_id: BarrierId) -> MsgData {
    let mut msg = [0u8; size_of::<MsgData>()];
    let s = &mut msg[..];

    s[..MSG_PREFIX_SIZE].clone_from_slice(MSG_PREFIX);
    let s = &mut s[MSG_PREFIX_SIZE..];

    let msg_type: u8 = match msg_type {
        MsgType::Reset => 0,
        MsgType::Step1 => 1,
        MsgType::Step2 => 2,
    };

    s[..MSG_TYPE_SIZE].clone_from_slice(&msg_type.to_le_bytes());
    let s = &mut s[MSG_TYPE_SIZE..];

    s[..MSG_ROUND_SIZE].clone_from_slice(&round.to_le_bytes());
    let s = &mut s[MSG_ROUND_SIZE..];

    s[..MSG_ID_SIZE].clone_from_slice(&barrier_id.to_le_bytes());
    let s = &mut s[MSG_ID_SIZE..];

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

    let (msg_type, rest) = rest.split_at(MSG_TYPE_SIZE);

    let msg_type = match u8::from_le_bytes(msg_type.try_into().unwrap()) {
        0 => MsgType::Reset,
        1 => MsgType::Step1,
        2 => MsgType::Step2,
        _ => return None,
    };

    let (round_data, rest) = rest.split_at(MSG_ROUND_SIZE);
    let (id_data, suffix) = rest.split_at(MSG_ID_SIZE);

    if suffix != MSG_SUFFIX {
        tracing::error!("Barrier: bad suffix");
        return None;
    }

    // Unwraps OK because we know the sizes at compile time.
    Some((
        msg_type,
        Round::from_le_bytes(round_data.try_into().unwrap()),
        BarrierId::from_le_bytes(id_data.try_into().unwrap()),
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
