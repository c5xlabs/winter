//! Development-only sample handoff: validation and the process-replacement port.

use std::convert::Infallible;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::process::Process;

/// Environment variable naming the absolute path of the development sample executable.
pub const SAMPLE_PATH_ENV: &str = "WINTER_DEV_SAMPLE_PATH";

/// The raw argument tail following `winter sample`, forwarded byte-for-byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleRequest {
    pub args: Vec<OsString>,
}

/// A request to replace the current process image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecRequest {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
}

/// Why the handoff to the sample could not complete.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("{SAMPLE_PATH_ENV} is not set or is empty")]
    MissingPath,
    #[error("{SAMPLE_PATH_ENV} must be an absolute path, got {}", .0.display())]
    RelativePath(PathBuf),
    #[error("cannot replace the process with {}: {source}", executable.display())]
    Replacement {
        executable: PathBuf,
        source: io::Error,
    },
}

/// Validates the sample path and replaces the process through `process`.
///
/// Returns only on failure. No filesystem preflight, canonicalization or PATH lookup occurs.
pub fn run_sample(
    request: SampleRequest,
    executable: Option<OsString>,
    process: &mut dyn Process,
) -> Result<Infallible, LaunchError> {
    let executable = match executable {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => return Err(LaunchError::MissingPath),
    };
    if !Path::new(&executable).is_absolute() {
        return Err(LaunchError::RelativePath(executable));
    }
    let exec = ExecRequest {
        executable: executable.clone(),
        args: request.args,
    };
    match process.replace(exec) {
        Ok(never) => match never {},
        Err(source) => Err(LaunchError::Replacement { executable, source }),
    }
}
