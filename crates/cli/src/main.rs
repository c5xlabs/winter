//! The `winter` executable: composition root and sole owner of the process streams.

use std::io::{self, Write};
use std::process::ExitCode;

use winter_platform::{CliIdentity, Reply, Request, control_reply, parse, usage_reply};

fn main() -> ExitCode {
    let identity = CliIdentity {
        version: env!("CARGO_PKG_VERSION"),
    };
    let reply = match parse(std::env::args_os(), &identity) {
        Ok(Request::Control(control)) => control_reply(control, identity),
        Err(error) => usage_reply(error),
    };
    let code = write_reply(&reply, &mut io::stdout().lock(), &mut io::stderr().lock());
    ExitCode::from(code)
}

/// Writes the reply and returns the process exit code; any stream failure yields `1`.
fn write_reply(reply: &Reply, out: &mut impl Write, err: &mut impl Write) -> u8 {
    if let Err(error) = out.write_all(&reply.stdout).and_then(|()| out.flush()) {
        let _ = writeln!(err, "winter: failed to write reply: {error}");
        let _ = err.flush();
        return 1;
    }
    if err
        .write_all(&reply.stderr)
        .and_then(|()| err.flush())
        .is_err()
    {
        return 1;
    }
    reply.exit_code
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailWrite;
    impl Write for FailWrite {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("boom"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailFlush;
    impl Write for FailFlush {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("boom"))
        }
    }

    fn reply(stdout: &str, stderr: &str, exit_code: u8) -> Reply {
        Reply {
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code,
        }
    }

    #[test]
    fn success_writes_exact_bytes_and_returns_code() {
        for (r, code) in [(reply("out\n", "", 0), 0), (reply("", "usage\n", 2), 2)] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            assert_eq!(write_reply(&r, &mut out, &mut err), code);
            assert_eq!(out, r.stdout);
            assert_eq!(err, r.stderr);
        }
    }

    #[test]
    fn stdout_failure_returns_one_with_diagnostic_only() {
        let r = reply("out\n", "should-not-appear", 0);
        let mut err = Vec::new();
        assert_eq!(write_reply(&r, &mut FailWrite, &mut err), 1);
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("failed to write reply"));
        assert!(!text.contains("should-not-appear"));

        let mut err = Vec::new();
        assert_eq!(write_reply(&r, &mut FailFlush, &mut err), 1);
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("failed to write reply"));
        assert!(!text.contains("should-not-appear"));
    }

    #[test]
    fn stderr_failure_returns_one_and_writes_nothing_to_stdout() {
        let r = reply("", "usage\n", 2);
        let mut out = Vec::new();
        assert_eq!(write_reply(&r, &mut out, &mut FailWrite), 1);
        assert!(out.is_empty());
    }
}
