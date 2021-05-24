use std::{io, sync::Once};

pub struct Logger;

impl Logger {
    pub fn new() -> Result<Self, io::Error> {
        static LOG_INIT: Once = Once::new();
        LOG_INIT.call_once(env_logger::init);

        Ok(Self)
    }
}
