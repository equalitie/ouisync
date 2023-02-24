//! Dart FFI (foreign function interface) for the ouisync library.

// Almost all functions in this crate are unsafe due to the nature of FFI
#![allow(clippy::missing_safety_doc)]

#[macro_use]
mod utils;

pub mod directory;
pub mod error;
pub mod file;
pub mod network;
pub mod repository;
pub mod session;
pub mod share_token;

mod client_message;
mod dart;
mod logger;
mod registry;
mod server;
mod server_message;
mod socket;
mod state;
mod state_monitor;
