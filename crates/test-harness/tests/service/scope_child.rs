//! Bounded owner for the regression's deliberately separate session.
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LIMIT: u64 = 1024 * 1024;
const POLL: Duration = Duration::from_millis(10);

struct GroupChild(Option<Child>);

impl GroupChild {
    fn kill_group(&mut self) -> io::Result<()> {
        let Some(child) = self.0.as_mut() else {
            return Ok(());
        };
        // The unreaped child is the session and group leader; never signal our group.
        let pid = libc::pid_t::try_from(child.id()).map_err(io::Error::other)?;
        if pid <= 0 || pid == unsafe { libc::getpgrp() } {
            return Err(io::Error::other("invalid owned process group"));
        }
        if unsafe { libc::kill(-pid, libc::SIGKILL) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> io::Result<()> {
        self.kill_group()?;
        let Some(child) = self.0.as_mut() else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if child.try_wait()?.is_some() {
                self.0 = None;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "group leader did not reap",
                ));
            }
            thread::sleep(POLL);
        }
    }
}

impl Drop for GroupChild {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn run(mut command: Command) -> io::Result<Output> {
    run_with_timeout(&mut command, Duration::from_secs(10))
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let dir = match std::env::var_os("BLIZZARD_TMPDIR") {
        Some(path) => tempfile::Builder::new()
            .prefix("winter-scope-")
            .tempdir_in(path)?,
        None => tempfile::Builder::new().prefix("winter-scope-").tempdir()?,
    };
    let out = dir.path().join("stdout");
    let err = dir.path().join("stderr");
    command
        .stdin(Stdio::null())
        .stdout(File::create(&out)?)
        .stderr(File::create(&err)?);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut guard = GroupChild(Some(command.spawn()?));
    let deadline = Instant::now() + timeout;
    loop {
        if std::fs::metadata(&out)?.len() > LIMIT || std::fs::metadata(&err)?.len() > LIMIT {
            guard.stop()?;
            return Err(io::Error::other("scope regression output limit exceeded"));
        }
        if Instant::now() >= deadline {
            guard.stop()?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "scope regression timed out",
            ));
        }
        // Observe exit without reaping: keep the group leader's PID reserved
        // until all remaining descendants have been killed, even if the shell
        // exits unexpectedly while its nested test is still running.
        let pid = guard.0.as_ref().unwrap().id();
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if unsafe { info.si_pid() } != 0 {
            guard.kill_group()?;
            let status = guard.0.as_mut().unwrap().wait()?;
            guard.0 = None;
            let read = |path| -> io::Result<Vec<u8>> {
                let mut bytes = Vec::new();
                File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
                if bytes.len() > LIMIT as usize {
                    return Err(io::Error::other("scope regression output limit exceeded"));
                }
                Ok(bytes)
            };
            return Ok(Output {
                status,
                stdout: read(out)?,
                stderr: read(err)?,
            });
        }
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_descendants_after_leader_exit_and_timeout() {
        super::super::inputs();
        let dir = tempfile::Builder::new()
            .prefix("winter-scope-check-")
            .tempdir_in(
                std::env::var_os("BLIZZARD_TMPDIR").map_or_else(std::env::temp_dir, Into::into),
            )
            .unwrap();
        for (script, timeout, timed_out) in [
            (
                "(/bin/sleep 0.4; printf escaped > \"$1\") & exit 0",
                Duration::from_secs(2),
                false,
            ),
            (
                "(/bin/sleep 0.4; printf escaped > \"$1\") & wait",
                Duration::from_millis(100),
                true,
            ),
        ] {
            let marker = dir.path().join(if timed_out { "timeout" } else { "exit" });
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script, "scope-cleanup"]).arg(&marker);
            let result = run_with_timeout(&mut command, timeout);
            if timed_out {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
            } else {
                assert!(result.unwrap().status.success());
            }
            thread::sleep(Duration::from_millis(500));
            assert!(!marker.exists(), "descendant survived group cleanup");
        }
    }

    #[test]
    fn rejects_excess_output() {
        super::super::inputs();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "/usr/bin/head -c 1100000 /dev/zero"]);
        let error = run_with_timeout(&mut command, Duration::from_secs(2)).unwrap_err();
        assert!(error.to_string().contains("output limit exceeded"));
    }
}
