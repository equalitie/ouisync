use super::{timer::Id, Context};
use std::{panic::Location, time::Duration};

/// Attach this to objects that are expected to be short-lived to be warned when they live longer
/// than expected.
pub(crate) struct ExpectShortLifetime {
    id: Id,
}

impl ExpectShortLifetime {
    pub fn new(max_lifetime: Duration, location: &'static Location<'static>) -> Self {
        let context = Context::new(location);
        let id = super::schedule(max_lifetime, context);

        Self { id }
    }
}

impl Drop for ExpectShortLifetime {
    fn drop(&mut self) {
        super::cancel(self.id);
    }
}
