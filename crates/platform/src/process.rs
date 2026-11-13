//! The process-replacement port and its Unix adapter (development sample only).

use std::convert::Infallible;
use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use crate::sample::{ExecRequest, SAMPLE_PATH_ENV};

/// Port: replaces the current process image. Returns only on failure.
pub trait Process {
    fn replace(&mut self, request: ExecRequest) -> io::Result<Infallible>;
}

/// Adapter that replaces the process with a raw `execve`.
///
/// `execvp` and `CommandExt::exec` are deliberately not used: neither may fall back to a shell
/// for `ENOEXEC`, and neither searches `PATH` for an absolute handoff.
#[derive(Debug, Default)]
pub struct UnixProcess;

/// Owned, NUL-free storage for `execve` arguments.
struct ExecStorage {
    executable: CString,
    argv: Vec<CString>,
    envp: Vec<CString>,
}

fn cstring(bytes: Vec<u8>) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "arguments and environment must not contain NUL bytes",
        )
    })
}

impl ExecStorage {
    fn new(
        request: ExecRequest,
        env: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> io::Result<Self> {
        let path = request.executable.into_os_string().into_vec();
        let mut argv = vec![cstring(path.clone())?];
        for arg in request.args {
            argv.push(cstring(arg.into_vec())?);
        }
        let mut envp = Vec::new();
        for (key, value) in env {
            if key.as_bytes() == SAMPLE_PATH_ENV.as_bytes() {
                continue;
            }
            let mut entry = key.into_vec();
            entry.push(b'=');
            entry.extend(value.into_vec());
            envp.push(cstring(entry)?);
        }
        Ok(Self {
            executable: cstring(path)?,
            argv,
            envp,
        })
    }
}

fn pointers(items: &[CString]) -> Vec<*const libc::c_char> {
    items
        .iter()
        .map(|item| item.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect()
}

impl Process for UnixProcess {
    fn replace(&mut self, request: ExecRequest) -> io::Result<Infallible> {
        // A snapshot: the global environment is never mutated.
        let storage = ExecStorage::new(request, std::env::vars_os())?;
        let argv = pointers(&storage.argv);
        let envp = pointers(&storage.envp);
        // The Rust runtime ignores SIGPIPE and ignored dispositions survive execve, so the
        // sample would inherit it. Reset to default for the handoff and restore on failure so
        // the launcher's failure reply still sees EPIPE rather than dying by signal.
        // SAFETY: plain libc calls on storage that outlives them; NULL-terminated vectors.
        unsafe {
            let previous = libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::execve(storage.executable.as_ptr(), argv.as_ptr(), envp.as_ptr());
            let error = io::Error::last_os_error();
            libc::signal(libc::SIGPIPE, previous);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn os(bytes: &[u8]) -> OsString {
        OsString::from_vec(bytes.to_vec())
    }

    fn request(args: Vec<OsString>) -> ExecRequest {
        ExecRequest {
            executable: PathBuf::from(os(b"/opt/s \xff/sample")),
            args,
        }
    }

    fn text(items: &[CString]) -> Vec<Vec<u8>> {
        items.iter().map(|c| c.as_bytes().to_vec()).collect()
    }

    #[test]
    fn argv_is_executable_then_exact_args() {
        let args = vec![
            os(b"inspect"),
            os(b""),
            os(b"--"),
            os(b"\xff\xfe"),
            os(b"a b"),
        ];
        let s = ExecStorage::new(request(args), []).unwrap();
        assert_eq!(s.executable.as_bytes(), b"/opt/s \xff/sample");
        assert_eq!(
            text(&s.argv),
            vec![
                b"/opt/s \xff/sample".to_vec(),
                b"inspect".to_vec(),
                b"".to_vec(),
                b"--".to_vec(),
                b"\xff\xfe".to_vec(),
                b"a b".to_vec()
            ]
        );
    }

    #[test]
    fn only_the_exact_sample_key_is_dropped() {
        let env = vec![
            (os(b"WINTER_DEV_SAMPLE_PATH"), os(b"/x")),
            (os(b"WINTER_DEV_SAMPLE_PATH_X"), os(b"1")),
            (os(b"winter_dev_sample_path"), os(b"2")),
            (os(b"KEEP"), os(b"\xff\xfe")),
        ];
        let s = ExecStorage::new(request(vec![]), env).unwrap();
        assert_eq!(
            text(&s.envp),
            vec![
                b"WINTER_DEV_SAMPLE_PATH_X=1".to_vec(),
                b"winter_dev_sample_path=2".to_vec(),
                b"KEEP=\xff\xfe".to_vec()
            ]
        );
    }

    #[test]
    fn nul_bytes_are_invalid_input() {
        let nul = |e: io::Result<ExecStorage>| e.err().unwrap().kind();
        let bad_exe = ExecRequest {
            executable: PathBuf::from(os(b"/a\0b")),
            args: vec![],
        };
        assert_eq!(
            nul(ExecStorage::new(bad_exe, [])),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            nul(ExecStorage::new(request(vec![os(b"a\0")]), [])),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            nul(ExecStorage::new(request(vec![]), [(os(b"K\0"), os(b"v"))])),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            nul(ExecStorage::new(request(vec![]), [(os(b"K"), os(b"v\0"))])),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn replace_with_nul_argument_never_reaches_execve() {
        let error = UnixProcess.replace(request(vec![os(b"a\0b")])).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
