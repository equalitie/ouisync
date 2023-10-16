mod tracker;

use crate::protocol::BlockId;

pub(crate) use tracker::{OfferState, PartPromise, Tracker, TrackerClient};

#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissingPartId {
    Block(BlockId),
}
