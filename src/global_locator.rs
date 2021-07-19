use crate::{Locator, ReplicaId};

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct GlobalLocator {
    pub branch_id: ReplicaId,
    pub local: Locator,
}
