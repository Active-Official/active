//! Active is a modern configuration and data language designed to be safer than
//! YAML, more expressive than TOML, more human-friendly than JSON, and usable by
//! both humans and AI agents.
//!
//! This crate is the language facade: the crate dependents use. The
//! implementation lives in [`active_core`]; everything public there is
//! re-exported here so `use active::...` paths stay stable.
//!
//! External-consumer entry point (CHECKLIST 1.009): parse and evaluate
//! without the monorepo. Depend on `active` from crates.io and drive this.
//!
//! ```rust
//! let module = active::parser::parse("port = 8080\n", "demo").unwrap();
//! let engine = active::Engine::new();
//! let (value, diags) = engine.evaluate(&module).unwrap();
//! assert!(diags.is_empty());
//! assert!(format!("{value:?}").contains("8080"));
//! ```

pub use active_core::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
