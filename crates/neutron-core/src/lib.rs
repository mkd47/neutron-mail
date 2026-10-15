//! Neutron Mail core library.
//!
//! Everything that talks to the network or the disk lives here so it can be
//! tested without a GUI. The Tauri shell (`src-tauri`) is a thin layer that
//! exposes these functions as commands.

pub mod attachments;
pub mod compose;
pub mod error;
pub mod folders;
pub mod gnome;
pub mod ical;
pub mod imap;
pub mod links;
pub mod managesieve;
pub mod model;
pub mod mutf7;
pub mod oauth;
pub mod parse;
pub mod providers;
pub mod remote;
pub mod rules;
pub mod rules_exec;
pub mod sanitize;
pub mod search;
pub mod secrets;
pub mod smtp;
pub mod store;
pub mod sync;
pub mod tls;

pub use error::{Error, Result};
