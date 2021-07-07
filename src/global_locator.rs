use crate::{Locator, ReplicaId};

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct GlobalLocator {
    pub branch: ReplicaId,
    pub local: Locator,
}
