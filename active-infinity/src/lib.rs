//! `active-infinity`: placeholder reserving the Active ecosystem name.
//!
//! This crate currently exposes only version identity. The real crate —
//! Active's unbounded and streaming evaluation surface — replaces this
//! placeholder in a later release, at which point the version converges
//! to the workspace version (see `Cargo.toml`).
//!
//! # Example
//!
//! ```rust
//! assert_eq!(active_infinity::VERSION, "1.0.0");
//! ```

/// Crate version (matches `[package] version`; converges to the workspace
/// version when the real crate lands).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_matches_package_manifest() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }
}
