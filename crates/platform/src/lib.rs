//! Winter platform: command parsing and reply rendering for the `winter` executable.
//!
//! The platform performs no I/O. Callers pass argument vectors and an executable identity and
//! receive [`Request`] and [`Reply`] values.

mod cli;

pub use cli::{
    CliIdentity, Control, Reply, Request, UsageError, control_reply, parse, usage_reply,
};
