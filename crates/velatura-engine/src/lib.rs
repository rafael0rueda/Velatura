//! Host access, plans and transactions for Velatura.
//!
//! Everything that reads or changes the machine goes through [`host`], so
//! tests can substitute a failing implementation and a sandboxed build can
//! substitute its own. This crate returns typed values; turning them into
//! text for people is the caller's job.

pub mod block;
pub mod companion;
pub mod cursor_set;
pub mod engine;
pub mod firefox;
#[cfg(feature = "gio")]
pub mod gio_settings;
pub mod gresource;
pub mod host;
pub mod icon_set;
pub mod keyfile;
pub mod library;
pub mod look;
mod off;
pub mod plan;
pub mod schedule;
pub mod store;
pub mod targets;
pub mod templates;
pub mod txn;
