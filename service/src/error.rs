use crate::state::StateError;
use ouisync_bridge::config::ConfigError;
use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("config error")]
    Config(#[from] ConfigError),
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("state error")]
    State(#[from] StateError),
}
