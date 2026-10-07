//! Library face of btrfs-win-driver.
//!
//! Exposes the modules the binary's `main.rs` consumes so the integration
//! tests under `tests/` can reach them too. No logic of its own.

pub mod mount;
pub mod probe;
