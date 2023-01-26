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

mod dart;
mod interface;
mod logger;
mod registry;
