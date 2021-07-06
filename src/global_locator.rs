use crate::{ ReplicaId, Locator };

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct GlobalLocator {
    pub branch: ReplicaId,
    pub local: Locator,
}
