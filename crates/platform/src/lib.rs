//! Winter platform: command parsing and reply rendering for the `winter` executable.
//!
//! The platform performs no I/O except the feature-gated `UnixProcess` replacement adapter.
//! Callers pass argument vectors and an executable identity and receive [`Request`] and
//! [`Reply`] values.
#![cfg_attr(
    not(feature = "dev-sample"),
    doc = "\nProduction builds do not contain the sample handoff:\n\n```compile_fail\nuse winter_platform::run_sample;\n```"
)]

mod cli;
#[cfg(feature = "dev-sample")]
mod process;
#[cfg(feature = "dev-sample")]
mod sample;

#[cfg(feature = "dev-sample")]
pub use cli::launch_failure_reply;
pub use cli::{
    CliIdentity, Control, Reply, Request, UsageError, control_reply, parse, usage_reply,
};
#[cfg(feature = "dev-sample")]
pub use process::{Process, UnixProcess};
#[cfg(feature = "dev-sample")]
pub use sample::{ExecRequest, LaunchError, SAMPLE_PATH_ENV, SampleRequest, run_sample};
