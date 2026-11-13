//! Bounded owner for the regression's deliberately separate session.
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LIMIT: u64 = 1024 * 1024;
const POLL: Duration = Duration::from_millis(10);

struct GroupChild(Option<Child>, Option<tempfile::TempDir>);

// ps works on both supported Unix hosts. Bound the inspector as well as its
// output; never wait indefinitely on a subprocess while cleaning up.
fn live_processes(deadline: Instant) -> io::Result<Vec<(libc::pid_t, libc::pid_t)>> {
    let mut file = tempfile::tempfile()?;
    let mut ps = Command::new("/bin/ps")
        .args(["-axo", "pid=,pgid=,stat="])
        .stdin(Stdio::null())
        .stdout(Stdio::from(file.try_clone()?))
        .stderr(Stdio::null())
        .spawn()?;
    let status = loop {
        match ps.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            result => {
                let _ = ps.kill();
                thread::spawn(move || {
                    let _ = ps.wait();
                });
                return Err(match result {
                    Err(error) => error,
                    _ => io::Error::new(io::ErrorKind::TimedOut, "process inspection timed out"),
                });
            }
        }
    };
    if !status.success() {
        return Err(io::Error::other("process inspection failed"));
    }
    file.rewind()?;
    let mut reader = BufReader::new(file.take(LIMIT + 1));
    let mut processes = Vec::new();
    let mut consumed = 0u64;
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process inspection timed out",
            ));
        }
        let mut line = String::new();
        let size = reader.read_line(&mut line)?;
        if size == 0 {
            break;
        }
        consumed += size as u64;
        if consumed > LIMIT {
            return Err(io::Error::other("process inspection output limit exceeded"));
        }
        let mut fields = line.split_whitespace();
        let pid = fields
            .next()
            .ok_or_else(|| io::Error::other("missing process ID"))?
            .parse()
            .map_err(io::Error::other)?;
        let group = fields
            .next()
            .ok_or_else(|| io::Error::other("missing process group"))?
            .parse()
            .map_err(io::Error::other)?;
        let state = fields
            .next()
            .ok_or_else(|| io::Error::other("missing process state"))?;
        if !state.starts_with('Z') && !state.starts_with('X') {
            processes.push((pid, group));
        }
    }
    Ok(processes)
}

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
            // macOS reports EPERM for a group whose only members are zombies; the
            // liveness check that follows still requires the group to be gone.
            if !matches!(error.raw_os_error(), Some(libc::ESRCH | libc::EPERM)) {
                return Err(error);
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.kill_group()?;
        let Some(child) = self.0.as_ref() else {
            return Ok(None);
        };
        let pid = libc::pid_t::try_from(child.id()).map_err(io::Error::other)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // Do not reap the leader until the group has stopped: its PID
            // reserves the group identity throughout observation.
            if !live_processes(deadline)?
                .iter()
                .any(|&(_, group)| group == pid)
            {
                let status = self
                    .0
                    .as_mut()
                    .unwrap()
                    .try_wait()?
                    .ok_or_else(|| io::Error::other("group leader still running"))?;
                self.0 = None;
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "process group did not stop",
                ));
            }
            thread::sleep(POLL);
        }
    }
}

impl Drop for GroupChild {
    fn drop(&mut self) {
        if self.stop().is_err() {
            // Panic/unwind paths must not delete a fixture still in use either.
            if let Some(dir) = self.1.take() {
                let _ = dir.keep();
            }
        }
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
    let mut guard = GroupChild(Some(command.spawn()?), Some(dir));
    let result = (|| {
        let deadline = Instant::now() + timeout;
        loop {
            if std::fs::metadata(&out)?.len() > LIMIT || std::fs::metadata(&err)?.len() > LIMIT {
                return Err(io::Error::other("scope regression output limit exceeded"));
            }
            if Instant::now() >= deadline {
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
                let status = guard
                    .stop()?
                    .ok_or_else(|| io::Error::other("missing group leader"))?;
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
    })();
    // Even unexpected I/O errors must clean up before the temporary directory
    // is removed. On failed cleanup retain it rather than expose a live writer
    // to a removed/reused fixture; Drop still makes an emergency attempt.
    if let Err(error) = guard.stop() {
        let path = guard.1.take().unwrap().keep();
        let original = result
            .as_ref()
            .err()
            .map(|error| format!("; original error: {error}"))
            .unwrap_or_default();
        return Err(io::Error::new(
            error.kind(),
            format!(
                "cleanup failed ({error}); fixture retained at {}{original}",
                path.display(),
            ),
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_descendants_before_return() {
        super::super::inputs();
        let dir = tempfile::Builder::new()
            .prefix("winter-scope-check-")
            .tempdir_in(
                std::env::var_os("BLIZZARD_TMPDIR").map_or_else(std::env::temp_dir, Into::into),
            )
            .unwrap();
        for (name, tail, timeout) in [
            ("exit", "exit 0", Duration::from_secs(2)),
            ("failure", "exit 42", Duration::from_secs(2)),
            ("timeout", "wait", Duration::from_millis(300)),
            (
                "output",
                "/usr/bin/head -c 1100000 /dev/zero; wait",
                Duration::from_secs(2),
            ),
        ] {
            let marker = dir.path().join(name);
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    &format!("/bin/sleep 5 & echo $! > \"$1\"; {tail}"),
                    "scope-cleanup",
                ])
                .arg(&marker);
            let result = run_with_timeout(&mut command, timeout);
            let pid: libc::pid_t = std::fs::read_to_string(&marker)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // Check immediately: a still-live published descendant is a failure,
            // regardless of whether it might exit later on its own.
            let live = live_processes(Instant::now() + Duration::from_secs(2)).unwrap();
            assert!(
                !live.iter().any(|&(found, _)| found == pid),
                "descendant {pid} still running"
            );
            match name {
                "exit" => assert!(result.unwrap().status.success()),
                "failure" => assert_eq!(result.unwrap().status.code(), Some(42)),
                "timeout" => assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut),
                "output" => assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("output limit exceeded")
                ),
                _ => unreachable!(),
            }
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
