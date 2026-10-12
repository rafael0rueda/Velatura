//! Host access, plans and transactions for Velatura.
//!
//! Phase 0 contains only the first piece of the host-access layer. Everything
//! that reads or changes the machine goes through [`host`], so tests can
//! substitute a failing implementation and a sandboxed build can substitute
//! its own.

pub mod host;
