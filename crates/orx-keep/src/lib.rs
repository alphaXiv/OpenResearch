//! A keeper holds one child process for `orx up`, so the child outlives a server restart and
//! the next server attaches to it. The keeper never parses what the child says: it retains
//! output under a policy, forwards whole input frames, and ends the child once no server has
//! attached for its orphan timeout. FROZEN.md is the contract between keeper and server
//! versions.

// The keeper's child inherits its environment by design.
#![allow(clippy::disallowed_methods)]

mod keeper;
pub mod wire;

#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

pub use keeper::run;
#[cfg(windows)]
pub use platform::private_stdio;
