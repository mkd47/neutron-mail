//! Neutron Mail core library.
//!
//! Everything that talks to the network or the disk lives here so it can be
//! tested without a GUI. The Tauri shell (`src-tauri`) is a thin layer that
//! exposes these functions as commands.

pub mod error;
pub mod gnome;
pub mod imap;
pub mod model;
pub mod parse;
pub mod sanitize;
pub mod search;
pub mod secrets;
pub mod smtp;
pub mod store;
pub mod sync;
pub mod tls;

pub use error::{Error, Result};
