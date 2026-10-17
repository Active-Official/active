//! Formatter for the Active configuration and data language.
//!
//! Parses Active documents and reprints them, which is what canonical layout and
//! editor format-on-save both need. This library re-exports the formatter entry
//! point and the parsing pieces the binary itself uses; the implementation
//! lives in [`active_core::fmt`](https://docs.rs/active-core).

pub use active_core::fmt::*;
pub use active_core::{parse_file, ParseOptions, Style};
