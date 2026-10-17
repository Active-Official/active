//! Active is a modern configuration and data language designed to be safer than
//! YAML, more expressive than TOML, more human-friendly than JSON, and usable by
//! both humans and AI agents.
//!
//! This crate is the language facade: the crate dependents use. The
//! implementation lives in [`active_core`]; everything public there is
//! re-exported here so `use active::...` paths stay stable.

pub use active_core::*;

/// The published version of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
