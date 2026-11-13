use serde_json::Value;
use std::ffi::OsString;
#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(15 * 60);
const REAP: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(10);
const EXCERPT: u64 = 64 * 1024;
// Bound individual JSON records and metadata while parsing the complete file.
const JSON_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Configuration {
    Production,
    Development,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    Unit,
    Component,
    Service,
}
#[derive(Debug, PartialEq, Eq)]
enum Task {
    Build(Configuration),
    Test(Tier, Configuration, Option<OsString>),
    Run(Vec<OsString>),
}

fn parse(args: &[OsString]) -> Result<Task, &'static str> {
    if args.first().is_some_and(|x| x == "run") {
        return if args.get(1).is_some_and(|x| x == "--") {
            Ok(Task::Run(args[2..].to_vec()))
        } else {
            Err("expected run -- [ARG...]")
        };
    }
    let (tier, mut i) = match args.first().and_then(|s| s.to_str()) {
        Some("build") => (None, 1),
        Some("test") => (
            Some(match args.get(1).and_then(|s| s.to_str()) {
                Some("unit") => Tier::Unit,
                Some("component") => Tier::Component,
                Some("service") => Tier::Service,
                _ => return Err("expected test unit|component|service"),
            }),
            2,
        ),
        _ => return Err("expected build, test, or run"),
    };
    let mut config = None;
    let mut filter = None;
    while i < args.len() {
        if args[i] == "--configuration" && config.is_none() {
            config = Some(match args.get(i + 1).and_then(|s| s.to_str()) {
                Some("production") => Configuration::Production,
                Some("development") => Configuration::Development,
                _ => return Err("expected --configuration production|development"),
            });
        } else if args[i] == "--filter" && tier.is_some() && filter.is_none() {
            filter = Some(args.get(i + 1).ok_or("expected filter text")?.clone());
        } else {
            return Err("invalid or duplicate option");
        }
        i += 2;
    }
    let config = config.ok_or("missing --configuration")?;
    Ok(match tier {
        Some(t) => Task::Test(t, config, filter),
        None => Task::Build(config),
    })
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("checkout exists")
}
fn cargo(root: &Path, config: Configuration, sub: &str) -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root)
        .arg(sub)
        .args(["--locked", "--offline", "--no-default-features"])
        .env(
            "CARGO_TARGET_DIR",
            root.join("target/verification").join(match config {
                Configuration::Production => "production",
                Configuration::Development => "development",
            }),
        );
    // Encoded flags take precedence over RUSTFLAGS. Normalize both without losing host flags.
    let mut flags = std::env::var_os("CARGO_ENCODED_RUSTFLAGS").unwrap_or_else(|| {
        std::env::var_os("RUSTFLAGS")
            .unwrap_or_default()
            .to_string_lossy()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("\x1f")
            .into()
    });
    if !flags.is_empty() {
        flags.push("\x1f");
    }
    flags.push("-Dwarnings");
    cmd.env("CARGO_ENCODED_RUSTFLAGS", flags);
    for key in ["RUSTFLAGS", "RUSTDOCFLAGS"] {
        let mut value = std::env::var_os(key).unwrap_or_default();
        value.push(" -Dwarnings");
        cmd.env(key, value);
    }
    cmd
}
fn features(cmd: &mut Command, config: Configuration, package: &str) {
    if config == Configuration::Development && package != "winter-dev-sample" {
        cmd.args(["--features", "dev-sample"]);
    }
}

// A group is created only for a direct child, and is never our own group.
struct Group {
    child: Child,
    pgid: libc::pid_t,
    reaped: bool,
    cleaned: bool,
}
impl Group {
    fn cleanup(&mut self) -> io::Result<()> {
        if self.cleaned {
            return Ok(());
        }
        if self.pgid <= 0 || self.pgid == unsafe { libc::getpgrp() } {
            return Err(io::Error::other("refusing unsafe process group"));
        }
        if unsafe { libc::kill(-self.pgid, libc::SIGKILL) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e);
            }
        }
        let until = Instant::now() + REAP;
        loop {
            if !self.reaped && self.child.try_wait()?.is_some() {
                self.reaped = true;
            }
            if self.reaped && !group_has_live_members(self.pgid, until)? {
                break;
            }
            if Instant::now() >= until {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "process group did not terminate or child did not reap",
                ));
            }
            thread::sleep(POLL);
        }
        self.cleaned = true;
        Ok(())
    }
}
// POSIX kill(pgid, 0) includes zombies, which an orphan's new parent must reap.
// ps is available on supported Unix hosts; its output goes to a file to avoid pipe
// deadlock, and the inspection itself shares the cleanup deadline.
fn group_has_live_members(pgid: libc::pid_t, deadline: Instant) -> io::Result<bool> {
    let file = tempfile::tempfile()?;
    let mut ps = Command::new("/bin/ps")
        .args(["-axo", "pgid=,stat="])
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
                // Reap asynchronously: even SIGKILL cannot guarantee that a
                // process stuck in the kernel exits before our deadline.
                thread::spawn(move || {
                    let _ = ps.wait();
                });
                return Err(match result {
                    Err(e) => e,
                    _ => io::Error::new(
                        io::ErrorKind::TimedOut,
                        "process group inspection timed out",
                    ),
                });
            }
        }
    };
    if !status.success() {
        return Err(io::Error::other("process group inspection failed"));
    }
    use std::io::Seek;
    let mut file = file;
    file.rewind()?;
    let mut reader = BufReader::new(file.take(JSON_LIMIT + 1));
    let mut consumed = 0u64;
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process group inspection timed out",
            ));
        }
        let mut line = String::new();
        let size = reader.read_line(&mut line)?;
        if size == 0 {
            break;
        }
        consumed += size as u64;
        if consumed > JSON_LIMIT {
            return Err(io::Error::other(
                "process group inspection output exceeds limit",
            ));
        }
        let mut fields = line.split_whitespace();
        let group = fields.next().and_then(|v| v.parse::<libc::pid_t>().ok());
        let state = fields.next();
        if group == Some(pgid) && state.is_some_and(|s| !s.starts_with('Z')) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn excerpt(path: &Path) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?.take(EXCERPT + 1).read_to_end(&mut data)?;
    if data.len() as u64 > EXCERPT {
        data.truncate(EXCERPT as usize);
        data.extend_from_slice(b"\n[output truncated]\n");
    }
    Ok(data)
}

impl Drop for Group {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            eprintln!("winter-dev: emergency cleanup: {e}");
        }
    }
}
#[derive(Debug)]
struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    _dir: tempfile::TempDir,
}
impl Output {
    fn stdout_file(&self) -> io::Result<File> {
        File::open(self._dir.path().join("out"))
    }
}
fn run(cmd: &mut Command, cancel: &AtomicBool, timeout: Duration) -> Result<Output, String> {
    let dir = match std::env::var_os("BLIZZARD_TMPDIR") {
        Some(p) => tempfile::Builder::new()
            .prefix("winter-driver-")
            .tempdir_in(p),
        None => tempfile::Builder::new().prefix("winter-driver-").tempdir(),
    }
    .map_err(|e| e.to_string())?;
    let out = dir.path().join("out");
    let err = dir.path().join("err");
    cmd.stdout(Stdio::from(File::create(&out).map_err(|e| e.to_string())?))
        .stderr(Stdio::from(File::create(&err).map_err(|e| e.to_string())?))
        .process_group(0);
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {cmd:?}: {e}"))?;
    let pgid = i32::try_from(child.id()).map_err(|_| "invalid child PID")?;
    let mut group = Group {
        child,
        pgid,
        reaped: false,
        cleaned: false,
    };
    let end = Instant::now() + timeout;
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break Err("cancelled".to_string());
        }
        match group.child.try_wait() {
            Ok(Some(status)) => {
                group.reaped = true;
                break Ok(status);
            }
            Err(e) => break Err(format!("wait failed: {e}")),
            Ok(None) => (),
        }
        if Instant::now() >= end {
            break Err("timed out".to_string());
        }
        thread::sleep(POLL);
    };
    let cleanup = group.cleanup();
    // Do not lose cleanup errors if reading diagnostics also fails.
    let stdout = excerpt(&out);
    let stderr = excerpt(&err);
    let mut errors = Vec::new();
    if let Err(e) = result.as_ref() {
        errors.push(e.clone());
    }
    if let Err(e) = cleanup {
        errors.push(format!("cleanup failed: {e}"));
    }
    if let Err(e) = &stdout {
        errors.push(format!("reading child stdout: {e}"));
    }
    if let Err(e) = &stderr {
        errors.push(format!("reading child stderr: {e}"));
    }
    let stdout = stdout.unwrap_or_default();
    let stderr = stderr.unwrap_or_default();
    if !errors.is_empty() {
        return Err(format!(
            "{}\n{}\n{}",
            errors.join("; "),
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        ));
    }
    Ok(Output {
        status: result.expect("checked result"),
        stdout,
        stderr,
        _dir: dir,
    })
}
fn checked(cmd: &mut Command, cancel: &AtomicBool) -> Result<Output, String> {
    let output = run(cmd, cancel, DEADLINE)?;
    if !output.status.success() {
        return Err(format!(
            "{cmd:?} failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output)
}
fn metadata(root: &Path, config: Configuration, cancel: &AtomicBool) -> Result<Value, String> {
    let mut cmd = cargo(root, config, "metadata");
    cmd.args(["--format-version", "1", "--no-deps"]);
    let output = checked(&mut cmd, cancel)?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    let mut data = Vec::new();
    output
        .stdout_file()
        .map_err(|e| e.to_string())?
        .take(JSON_LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > JSON_LIMIT {
        return Err("Cargo metadata exceeds JSON limit".into());
    }
    serde_json::from_slice(&data).map_err(|e| format!("invalid Cargo metadata: {e}"))
}
fn package<'a>(metadata: &'a Value, name: &str) -> Result<&'a Value, String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("metadata missing packages")?;
    let matches: Vec<_> = packages.iter().filter(|p| p["name"] == name).collect();
    if matches.len() != 1 {
        return Err(format!("expected exactly one {name} package"));
    }
    Ok(matches[0])
}
fn artifact(reader: impl Read, id: &str, name: &str, root: &Path) -> Result<PathBuf, String> {
    let mut found = None;
    let mut count = 0usize;
    let mut rendered_bytes = 0usize;
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = Vec::new();
        let size = (&mut reader)
            .take(JSON_LIMIT + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if size == 0 {
            break;
        }
        if size as u64 > JSON_LIMIT {
            return Err("Cargo JSON record exceeds limit".into());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let msg: Value =
            serde_json::from_slice(&line).map_err(|e| format!("invalid Cargo JSON: {e}"))?;
        if msg["reason"] == "compiler-message"
            && let Some(rendered) = msg["message"]["rendered"].as_str()
        {
            let bytes = rendered.as_bytes();
            let remaining = (EXCERPT as usize).saturating_sub(rendered_bytes);
            let take = remaining.min(bytes.len());
            eprint!("{}", String::from_utf8_lossy(&bytes[..take]));
            rendered_bytes += take;
            if take < bytes.len() && remaining > 0 {
                eprintln!("\n[output truncated]");
            }
        }
        if msg["reason"] == "compiler-artifact"
            && msg["package_id"] == id
            && msg["target"]["name"] == name
            && msg["target"]["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|x| x == "bin"))
        {
            let path = msg["executable"]
                .as_str()
                .ok_or("matching artifact lacks executable")?;
            let path = PathBuf::from(path);
            count += 1;
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            if found.is_none() {
                found = Some(path);
            }
        }
    }
    if count != 1 {
        return Err(format!(
            "expected exactly one executable artifact for {name}, found {}",
            count
        ));
    }
    Ok(found.expect("one artifact"))
}
fn build_one(
    root: &Path,
    config: Configuration,
    meta: &Value,
    pkg: &str,
    bin: &str,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let id = package(meta, pkg)?["id"]
        .as_str()
        .ok_or("package missing id")?;
    let mut cmd = cargo(root, config, "build");
    cmd.args(["-p", pkg, "--bin", bin, "--message-format=json"]);
    features(&mut cmd, config, pkg);
    let output = checked(&mut cmd, cancel)?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    artifact(
        output.stdout_file().map_err(|e| e.to_string())?,
        id,
        bin,
        root,
    )
}
struct Artifacts {
    cli: PathBuf,
    sample: Option<PathBuf>,
    version: String,
}
fn exec_cli(artifacts: Artifacts, args: Vec<OsString>) -> Result<(), String> {
    if !artifacts.cli.is_absolute() {
        return Err("CLI artifact path is not absolute".into());
    }
    let sample = artifacts.sample.ok_or("missing sample")?;
    if !sample.is_absolute() {
        return Err("sample artifact path is not absolute".into());
    }
    let mut cmd = Command::new(&artifacts.cli);
    cmd.args(args).env("WINTER_DEV_SAMPLE_PATH", sample);
    Err(format!(
        "exec {} failed: {}",
        artifacts.cli.display(),
        cmd.exec()
    ))
}
fn build(root: &Path, config: Configuration, cancel: &AtomicBool) -> Result<Artifacts, String> {
    let meta = metadata(root, config, cancel)?;
    let version = package(&meta, "winter-cli")?["version"]
        .as_str()
        .ok_or("package missing version")?
        .to_owned();
    let cli = build_one(root, config, &meta, "winter-cli", "winter", cancel)?;
    let sample = if config == Configuration::Development {
        Some(build_one(
            root,
            config,
            &meta,
            "winter-dev-sample",
            "winter-dev-sample",
            cancel,
        )?)
    } else {
        None
    };
    Ok(Artifacts {
        cli,
        sample,
        version,
    })
}
fn test(
    root: &Path,
    tier: Tier,
    config: Configuration,
    filter: Option<OsString>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let artifacts = if tier == Tier::Service {
        Some(build(root, config, cancel)?)
    } else {
        None
    };
    let packages: &[&str] = if tier == Tier::Unit {
        if config == Configuration::Development {
            &[
                "winter-platform",
                "winter-cli",
                "winter-test-harness",
                "winter-dev-sample",
            ]
        } else {
            &["winter-platform", "winter-cli", "winter-test-harness"]
        }
    } else {
        &["winter-test-harness"]
    };
    for pkg in packages {
        let mut cmd = cargo(root, config, "test");
        cmd.args(["-p", pkg]);
        features(&mut cmd, config, pkg);
        match tier {
            Tier::Unit => {
                if *pkg == "winter-cli" {
                    cmd.args(["--bin", "winter"]);
                } else if *pkg == "winter-dev-sample" {
                    cmd.args(["--bin", "winter-dev-sample"]);
                } else if *pkg == "winter-test-harness" {
                    cmd.args(["--lib", "--bin", "winter-dev"]);
                } else if *pkg == "winter-platform" {
                    cmd.arg("--lib");
                }
            }
            Tier::Component => {
                cmd.args(["--test", "component"]);
            }
            Tier::Service => {
                cmd.args(["--test", "service"]);
            }
        }
        if let Some(a) = &artifacts {
            if !a.cli.is_absolute()
                || !a.cli.is_file()
                || a.version.is_empty()
                || a.sample
                    .as_ref()
                    .is_some_and(|p| !p.is_absolute() || !p.is_file())
            {
                return Err("invalid service artifact inputs".into());
            }
            cmd.env("WINTER_TEST_CLI_PATH", &a.cli)
                .env("WINTER_TEST_EXPECTED_VERSION", &a.version)
                .env("WINTER_TEST_PROCESS_SCOPE", std::process::id().to_string());
            if let Some(sample) = &a.sample {
                cmd.env("WINTER_TEST_SAMPLE_PATH", sample);
            } else {
                cmd.env_remove("WINTER_TEST_SAMPLE_PATH");
            }
        }
        if let Some(f) = &filter {
            cmd.arg(f);
        }
        let output = checked(&mut cmd, cancel)?;
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        if tier == Tier::Unit && (*pkg == "winter-test-harness" || *pkg == "winter-platform") {
            let mut doc = cargo(root, config, "test");
            doc.args(["-p", pkg, "--doc"]);
            features(&mut doc, config, pkg);
            if let Some(f) = &filter {
                doc.arg(f);
            }
            let output = checked(&mut doc, cancel)?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        }
    }
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let task = match parse(&args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("winter-dev: {e}");
            std::process::exit(2);
        }
    };
    let cancel = Arc::new(AtomicBool::new(false));
    for sig in [libc::SIGINT, libc::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(sig, Arc::clone(&cancel)) {
            eprintln!("winter-dev: signal setup: {e}");
            std::process::exit(1);
        }
    }
    let root = root();
    let result = match task {
        Task::Build(c) => build(&root, c, &cancel).map(|_| ()),
        Task::Test(t, c, f) => test(&root, t, c, f, &cancel),
        Task::Run(args) => build(&root, Configuration::Development, &cancel).and_then(|a| {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            exec_cli(a, args)
        }),
    };
    if let Err(e) = result {
        eprintln!("winter-dev: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicBool;
    use winter_test_harness::{Completion, RuntimeFixture, Termination, TestSignal, invoke};

    fn executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    // This test-only subprocess entry exercises the actual exec seam, not a public driver option.
    #[test]
    fn exec_helper() {
        let Some(target) = std::env::var_os("WINTER_EXEC_TEST_TARGET") else {
            return;
        };
        let result = exec_cli(
            Artifacts {
                cli: target.into(),
                sample: Some("/selected sample".into()),
                version: String::new(),
            },
            vec![
                OsString::new(),
                "a b".into(),
                "--option".into(),
                OsString::from_vec(b"bad-\xff".to_vec()),
            ],
        );
        eprintln!("winter-dev: {}", result.unwrap_err());
        std::process::exit(1);
    }

    #[test]
    fn exec_handoff_preserves_process_context_and_failure() {
        let fixture = RuntimeFixture::new().unwrap();
        let target = fixture.path().join("target");
        executable(
            &target,
            "#!/bin/sh\nprintf '%s\\0' \"$$\" \"$PWD\" \"$WINTER_DEV_SAMPLE_PATH\" \"$PROBE\" \"$@\"\n/bin/cat\nprintf diagnostic >&2\nexit 37\n",
        );
        let mut call = fixture.invocation(std::env::current_exe().unwrap());
        call.args = args(&["--exact", "tests::exec_helper", "--nocapture"]);
        call.env.insert(
            "WINTER_EXEC_TEST_TARGET".into(),
            target.clone().into_os_string(),
        );
        call.env.insert("PROBE".into(), "preserved".into());
        call.stdin = b"stdin-\xff".to_vec();
        let result = invoke(call).unwrap();
        let expected = format!(
            "{}\0{}\0/selected sample\0preserved\0\0a b\0--option\0",
            result.spawned_pid,
            fixture.path().join("cwd").display()
        );
        assert!(
            result
                .stdout
                .windows(expected.len())
                .any(|s| s == expected.as_bytes())
        );
        assert!(result.stdout.ends_with(b"bad-\xff\0stdin-\xff"));
        assert_eq!(result.stderr, b"diagnostic");
        assert_eq!(result.termination, Termination::Code(37));

        for (signal, number) in [
            (TestSignal::Interrupt, libc::SIGINT),
            (TestSignal::Terminate, libc::SIGTERM),
        ] {
            executable(&target, "#!/bin/sh\nprintf ready\nexec /bin/sleep 10\n");
            let mut call = fixture.invocation(std::env::current_exe().unwrap());
            call.args = args(&["--exact", "tests::exec_helper", "--nocapture"]);
            call.env.insert(
                "WINTER_EXEC_TEST_TARGET".into(),
                target.clone().into_os_string(),
            );
            call.completion = Completion::SignalAfterStdout {
                marker: b"ready".to_vec(),
                signal,
            };
            assert_eq!(
                invoke(call).unwrap().termination,
                Termination::Signal(number)
            );
        }
        let mut call = fixture.invocation(std::env::current_exe().unwrap());
        call.args = args(&["--exact", "tests::exec_helper", "--nocapture"]);
        call.env.insert(
            "WINTER_EXEC_TEST_TARGET".into(),
            fixture.path().join("missing").into_os_string(),
        );
        let failure = invoke(call).unwrap();
        assert_eq!(failure.termination, Termination::Code(1));
        assert!(String::from_utf8_lossy(&failure.stderr).contains("winter-dev: exec "));
    }

    #[test]
    fn wrapper_passes_raw_arguments_and_context_to_cargo() {
        let fixture = RuntimeFixture::new().unwrap();
        let cargo = fixture.path().join("bin/cargo");
        executable(
            &cargo,
            "#!/bin/bash\nprintf '%s\\0' \"$$\" \"$PWD\" \"$@\"\n/bin/cat\nprintf cargo-stderr >&2\nexit 23\n",
        );
        let mut call = fixture.invocation(root().join("scripts/winter-cli"));
        call.args = vec![
            OsString::new(),
            "a b".into(),
            "--option".into(),
            OsString::from_vec(b"bad-\xff".to_vec()),
        ];
        call.stdin = b"input-\xff".to_vec();
        // The wrapper needs Bash's dirname in addition to the controlled Cargo.
        call.env.insert(
            "PATH".into(),
            format!("{}:/usr/bin:/bin", fixture.path().join("bin").display()).into(),
        );
        let result = invoke(call).unwrap();
        let mut expected = Vec::new();
        let manifest = root().join("Cargo.toml");
        let channel = fs::read_to_string(root().join("rust-toolchain.toml")).unwrap();
        let channel = channel
            .lines()
            .find_map(|l| {
                l.strip_prefix("channel = \"")
                    .and_then(|s| s.strip_suffix('"'))
            })
            .unwrap();
        let pid = result.spawned_pid.to_string();
        let version = format!("+{channel}");
        for arg in [
            pid.as_bytes(),
            fixture.path().join("cwd").as_os_str().as_bytes(),
            version.as_bytes(),
            b"run",
            b"--locked",
            b"--offline",
            b"--manifest-path",
            manifest.as_os_str().as_bytes(),
            b"-p",
            b"winter-test-harness",
            b"--bin",
            b"winter-dev",
            b"--",
            b"run",
            b"--",
            b"",
            b"a b",
            b"--option",
            b"bad-\xff",
        ] {
            expected.extend_from_slice(arg);
            expected.push(0);
        }
        expected.extend_from_slice(b"input-\xff");
        assert_eq!(result.stdout, expected);
        assert_eq!(result.stderr, b"cargo-stderr");
        assert_eq!(result.termination, Termination::Code(23));
    }

    // The nested driver creates its own Cargo process groups. Keep the wrapper as a
    // direct child, let its SIGTERM handler clean those groups, and retain an
    // independent emergency path even on assertion failure or panic.
    struct NestedGuard {
        child: Child,
        pid: libc::pid_t,
        reaped: bool,
    }
    impl NestedGuard {
        fn start(cmd: &mut Command) -> Self {
            let child = cmd.process_group(0).spawn().unwrap();
            let pid = libc::pid_t::try_from(child.id()).unwrap();
            assert!(pid > 0 && pid != unsafe { libc::getpgrp() });
            Self {
                child,
                pid,
                reaped: false,
            }
        }
        fn wait_until(&mut self, deadline: Instant) -> Option<ExitStatus> {
            while Instant::now() < deadline {
                if let Some(status) = self.child.try_wait().unwrap() {
                    self.reaped = true;
                    return Some(status);
                }
                thread::sleep(POLL);
            }
            None
        }
        fn cancel(&mut self) -> Option<ExitStatus> {
            if unsafe { libc::kill(self.pid, libc::SIGTERM) } != 0 {
                assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
            }
            self.wait_until(Instant::now() + Duration::from_secs(3))
        }
        fn emergency(&mut self) -> Vec<libc::pid_t> {
            let mut killed = Vec::new();
            if self.reaped {
                return killed;
            }
            // Freeze the direct child before enumerating its direct children, so
            // it cannot spawn/reap and recycle a recorded process-group leader.
            if unsafe { libc::kill(self.pid, libc::SIGSTOP) } == 0 {
                let until = Instant::now() + Duration::from_secs(2);
                let mut status = 0;
                let mut stopped = false;
                while Instant::now() < until {
                    let result = unsafe {
                        libc::waitpid(self.pid, &mut status, libc::WUNTRACED | libc::WNOHANG)
                    };
                    if result == self.pid {
                        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                            self.reaped = true;
                            return killed;
                        }
                        if libc::WIFSTOPPED(status) {
                            stopped = true;
                            break;
                        }
                    }
                    thread::sleep(POLL);
                }
                let mut ps = Command::new("ps");
                ps.args(["-axo", "pid=,ppid=,pgid="]);
                let output = if stopped {
                    run(&mut ps, &AtomicBool::new(false), Duration::from_secs(2))
                        .ok()
                        .filter(|output| output.status.success())
                        .map(|output| output.stdout)
                } else {
                    None
                };
                if output.is_none() {
                    eprintln!("could not enumerate frozen nested driver groups");
                }
                for line in String::from_utf8_lossy(output.as_deref().unwrap_or_default()).lines() {
                    let fields: Vec<libc::pid_t> = line
                        .split_whitespace()
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if let [pid, parent, pgid] = fields.as_slice()
                        && *parent == self.pid
                        && *pid == *pgid
                        && *pid > 0
                        && *pid != unsafe { libc::getpgrp() }
                    {
                        unsafe {
                            libc::kill(-*pid, libc::SIGKILL);
                        }
                        killed.push(*pid);
                    }
                }
            }
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
            if self
                .wait_until(Instant::now() + Duration::from_secs(3))
                .is_none()
            {
                eprintln!("nested wrapper did not reap after emergency cleanup");
            }
            killed
        }
    }
    impl Drop for NestedGuard {
        fn drop(&mut self) {
            if !self.reaped && self.cancel().is_none() {
                let _ = self.emergency();
            }
        }
    }

    fn nested_command(
        fixture: &RuntimeFixture,
        name: &str,
        program: &Path,
    ) -> (Command, PathBuf, PathBuf) {
        let out = fixture.path().join(format!("{name}-out"));
        let err = fixture.path().join(format!("{name}-err"));
        let mut cmd = Command::new(program);
        let mut path = fixture.path().join("bin").into_os_string();
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        cmd.current_dir(fixture.path().join("cwd"))
            .env("PATH", path)
            .stdin(Stdio::null())
            .stdout(File::create(&out).unwrap())
            .stderr(File::create(&err).unwrap());
        (cmd, out, err)
    }

    #[test]
    fn nested_timeout_cancels_driver_and_live_cargo_group() {
        exercise_nested_cleanup(false);
    }

    #[test]
    fn nested_emergency_guard_kills_exact_live_cargo_group() {
        exercise_nested_cleanup(true);
    }

    fn exercise_nested_cleanup(emergency: bool) {
        let fixture = RuntimeFixture::new().unwrap();
        let proxy = fixture.path().join("cargo-proxy");
        let ready = fixture.path().join("ready");
        let escaped = fixture.path().join("escaped");
        executable(
            &proxy,
            "#!/bin/sh\n(/bin/sleep 1; printf escaped > \"$ESCAPED\") &\nprintf '%s %s' \"$$\" \"$!\" > \"$READY\"\nwait\n",
        );
        let cancel = AtomicBool::new(false);
        let meta = metadata(&root(), Configuration::Development, &cancel).unwrap();
        let driver = build_one(
            &root(),
            Configuration::Development,
            &meta,
            "winter-test-harness",
            "winter-dev",
            &cancel,
        )
        .unwrap();
        let (mut cmd, _, err) = nested_command(&fixture, "timeout", &driver);
        cmd.env("CARGO", proxy)
            .env("READY", &ready)
            .env("ESCAPED", &escaped)
            .args(["run", "--", "--version"]);
        let mut guard = NestedGuard::start(&mut cmd);
        let until = Instant::now() + Duration::from_secs(60);
        while !ready.exists() && Instant::now() < until {
            assert!(
                guard.child.try_wait().unwrap().is_none(),
                "driver exited before readiness: {}",
                String::from_utf8_lossy(&fs::read(&err).unwrap())
            );
            thread::sleep(POLL);
        }
        let ids = fs::read_to_string(&ready).expect("nested cargo readiness");
        let ids: Vec<i32> = ids.split_whitespace().map(|s| s.parse().unwrap()).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(unsafe { libc::getpgid(ids[0]) }, ids[0]);
        assert_eq!(unsafe { libc::getpgid(ids[1]) }, ids[0]);
        assert_ne!(ids[0], unsafe { libc::getpgrp() });
        if emergency {
            assert_eq!(
                guard.emergency(),
                vec![ids[0]],
                "must target only the owned nested group"
            );
            assert!(guard.reaped, "emergency cleanup must reap the driver");
        } else {
            // An expired test deadline follows this same cooperative cancellation path.
            assert!(guard.wait_until(Instant::now()).is_none());
            let status = guard
                .cancel()
                .expect("nested driver did not clean up before deadline");
            assert!(!status.success());
            assert!(String::from_utf8_lossy(&fs::read(err).unwrap()).contains("cancelled"));
        }
        thread::sleep(Duration::from_millis(1200));
        assert!(
            !escaped.exists(),
            "live nested descendant survived cancellation"
        );
    }

    #[test]
    fn real_wrapper_uses_checkout_from_unrelated_directory() {
        let fixture = RuntimeFixture::new().unwrap();
        fs::write(
            fixture.path().join("cwd/rust-toolchain.toml"),
            "[toolchain]\nchannel = \"nonexistent-conflicting-toolchain\"\n",
        )
        .unwrap();
        let marker = fixture.path().join("decoy-ran");
        executable(
            &fixture.path().join("bin/winter"),
            &format!("#!/bin/sh\nprintf bad > '{}'\nexit 99\n", marker.display()),
        );
        let meta = metadata(&root(), Configuration::Development, &AtomicBool::new(false)).unwrap();
        let version = package(&meta, "winter-cli").unwrap()["version"]
            .as_str()
            .unwrap();
        for (arguments, status, stream, expected) in [
            (vec!["--version"], 0, true, format!("winter {version}")),
            (vec!["--help"], 0, true, "Usage:".to_owned()),
            (vec!["--not-a-real-option"], 2, false, "error".to_owned()),
        ] {
            let (mut cmd, out, err) = nested_command(
                &fixture,
                &format!("run-{status}-{stream}"),
                &root().join("scripts/winter-cli"),
            );
            cmd.args(arguments);
            let mut guard = NestedGuard::start(&mut cmd);
            let observed = guard
                .wait_until(Instant::now() + Duration::from_secs(180))
                .unwrap_or_else(|| {
                    let _ = guard.cancel();
                    panic!("nested wrapper timed out")
                });
            assert_eq!(
                observed.code(),
                Some(status),
                "{}",
                String::from_utf8_lossy(&fs::read(&err).unwrap())
            );
            let output = if stream {
                fs::read(out).unwrap()
            } else {
                fs::read(err).unwrap()
            };
            assert!(
                String::from_utf8_lossy(&output).contains(&expected),
                "{}",
                String::from_utf8_lossy(&output)
            );
            assert!(!marker.exists());
        }
    }

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }
    #[test]
    fn command_selection_and_usage() {
        assert_eq!(
            parse(&args(&[
                "test",
                "component",
                "--filter",
                "foo",
                "--configuration",
                "development"
            ])),
            Ok(Task::Test(
                Tier::Component,
                Configuration::Development,
                Some("foo".into())
            ))
        );
        assert_eq!(
            parse(&args(&["build", "--configuration", "production"])),
            Ok(Task::Build(Configuration::Production))
        );
        assert_eq!(
            parse(&args(&["run", "--", "--help"])),
            Ok(Task::Run(vec!["--help".into()]))
        );
        for invalid in [
            vec![],
            vec!["test", "unit"],
            vec!["build", "--configuration", "bad"],
            vec!["run"],
            vec!["build", "--configuration", "production", "--filter", "x"],
            vec![
                "test",
                "unit",
                "--configuration",
                "production",
                "--configuration",
                "development",
            ],
        ] {
            assert!(parse(&args(&invalid)).is_err());
        }
    }
    #[test]
    fn artifact_selection_is_exact_and_strict() {
        let line = |id: &str, name: &str, path: &str| {
            format!(
                "{{\"reason\":\"compiler-artifact\",\"package_id\":\"{id}\",\"target\":{{\"name\":\"{name}\",\"kind\":[\"bin\"]}},\"executable\":\"{path}\"}}\n"
            )
        };
        let data = format!(
            "{}{}",
            line("other", "winter", "/wrong"),
            line("exact", "winter", "target/real")
        );
        assert_eq!(
            artifact(data.as_bytes(), "exact", "winter", Path::new("/checkout")).unwrap(),
            Path::new("/checkout/target/real")
        );
        assert!(artifact(data.as_bytes(), "missing", "winter", Path::new("/")).is_err());
        assert!(
            artifact(
                format!("{data}{}", line("exact", "winter", "/duplicate")).as_bytes(),
                "exact",
                "winter",
                Path::new("/")
            )
            .is_err()
        );
        assert!(artifact(&b"not json"[..], "exact", "winter", Path::new("/")).is_err());
        assert!(artifact(&br#"{"reason":"compiler-artifact","package_id":"exact","target":{"name":"winter","kind":["bin"]},"executable":null}"#[..], "exact", "winter", Path::new("/")).is_err());
    }
    #[test]
    fn metadata_package_selection_is_unique() {
        let m = serde_json::json!({"packages":[{"name":"winter-cli","id":"a","version":"42"}]});
        assert_eq!(package(&m, "winter-cli").unwrap()["version"], "42");
        assert!(package(&m, "other").is_err());
        let m = serde_json::json!({"packages":[{"name":"a"},{"name":"a"}]});
        assert!(package(&m, "a").is_err());
    }
    #[test]
    fn noisy_child_has_bounded_diagnostics_and_full_json_file() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args([
            "-c",
            "head -c 200000 /dev/zero; head -c 200000 /dev/zero >&2; exit 7",
        ]);
        let error = checked(&mut cmd, &AtomicBool::new(false)).unwrap_err();
        assert!(error.len() < 2 * EXCERPT as usize + 1024);
        assert_eq!(error.matches("[output truncated]").count(), 2);
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "i=0; while [ $i -lt 5000 ]; do printf '{\"reason\":\"other\"}\n'; i=$((i+1)); done; printf '{\"reason\":\"compiler-artifact\",\"package_id\":\"id\",\"target\":{\"name\":\"bin\",\"kind\":[\"bin\"]},\"executable\":\"/found\"}\n'"]);
        let output = checked(&mut cmd, &AtomicBool::new(false)).unwrap();
        assert!(output.stdout.ends_with(b"[output truncated]\n"));
        assert_eq!(
            artifact(output.stdout_file().unwrap(), "id", "bin", Path::new("/")).unwrap(),
            Path::new("/found")
        );
    }

    #[test]
    fn timeout_and_cancel_retain_output() {
        let flag = AtomicBool::new(false);
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "printf diagnostic >&2; exec /bin/sleep 5"]);
        let err = run(&mut cmd, &flag, Duration::from_millis(100))
            .err()
            .unwrap();
        assert!(err.contains("timed out") && err.contains("diagnostic"));
        let flag = Arc::new(AtomicBool::new(false));
        let f = Arc::clone(&flag);
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            f.store(true, Ordering::Relaxed);
        });
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "printf started; exec /bin/sleep 5"]);
        let err = run(&mut cmd, &flag, Duration::from_secs(5)).err().unwrap();
        setter.join().unwrap();
        assert!(err.contains("cancelled") && err.contains("started"));
    }
    #[test]
    fn group_cleanup_stops_descendants_even_after_parent_exit() {
        fn live(pid: libc::pid_t) -> bool {
            let status = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .expect("inspect descendant state");
            let state = String::from_utf8_lossy(&status.stdout);
            !state.trim().is_empty() && !state.trim_start().starts_with('Z')
        }
        // This guard is independent of Group, and remains outside the fixture's
        // group. It also runs on assertion failure when cleanup is broken.
        struct Emergency(PathBuf);
        impl Drop for Emergency {
            fn drop(&mut self) {
                let Ok(data) = fs::read_to_string(&self.0) else {
                    return;
                };
                let ids: Vec<libc::pid_t> = data
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if let [pgid, descendant] = ids.as_slice()
                    && *pgid > 0
                    && *descendant > 0
                    && *pgid != unsafe { libc::getpgrp() }
                    && unsafe { libc::getpgid(*descendant) } == *pgid
                {
                    unsafe { libc::kill(-*pgid, libc::SIGKILL) };
                    let deadline = Instant::now() + REAP;
                    while live(*descendant) && Instant::now() < deadline {
                        thread::sleep(POLL);
                    }
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let ids = dir.path().join("ids");
        let _emergency = Emergency(ids.clone());
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("/bin/sleep 30 & child=$!; printf '%s %s\\n' \"$$\" \"$child\" > \"$1\"; kill -0 \"$child\" || exit 1; exit 0")
            .arg("sh")
            .arg(&ids);
        assert!(
            run(&mut cmd, &AtomicBool::new(false), Duration::from_secs(3))
                .unwrap()
                .status
                .success()
        );
        let data = fs::read_to_string(&ids).unwrap();
        let ids: Vec<libc::pid_t> = data
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0] > 0 && ids[1] > 0 && ids[0] != unsafe { libc::getpgrp() });
        assert!(
            !group_has_live_members(ids[0], Instant::now() + REAP).unwrap(),
            "cleanup returned while group still had live members"
        );
        let deadline = Instant::now() + REAP;
        loop {
            // An orphan may remain as a zombie until init reaps it; it is not live.
            if !live(ids[1]) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "orphan descendant {} remains live in group {}",
                ids[1],
                ids[0]
            );
            thread::sleep(POLL);
        }
    }
    #[test]
    fn child_and_fixture_inherit_owned_group() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "ps -o pgid= -p $$; /bin/sh -c 'ps -o pgid= -p $$'"]);
        let output = run(&mut cmd, &AtomicBool::new(false), Duration::from_secs(3)).unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let groups: Vec<i32> = text.lines().map(|s| s.trim().parse().unwrap()).collect();
        assert_eq!(groups.len(), 2);
        assert!(groups[0] > 0);
        assert_ne!(groups[0], unsafe { libc::getpgrp() });
        assert_eq!(groups[0], groups[1]);
    }

    #[test]
    fn failing_parent_also_cleans_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("(/bin/sleep 0.3; printf bad > \"$1\") & exit 7")
            .arg("sh")
            .arg(&marker);
        assert!(
            !run(&mut cmd, &AtomicBool::new(false), Duration::from_secs(3))
                .unwrap()
                .status
                .success()
        );
        thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists());
    }
    #[test]
    fn group_guard_rejects_own_group() {
        let child = Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let mut guard = Group {
            child,
            pgid: unsafe { libc::getpgrp() },
            reaped: false,
            cleaned: false,
        };
        assert!(guard.cleanup().is_err());
        guard.child.kill().unwrap();
        guard.child.wait().unwrap();
        guard.cleaned = true;
    }

    #[test]
    fn emergency_guard_cleans_group_on_unwind() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("(/bin/sleep 0.3; printf bad > \"$1\") & wait")
            .arg("sh")
            .arg(&marker)
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        let result = std::panic::catch_unwind(|| {
            let _guard = Group {
                child,
                pgid,
                reaped: false,
                cleaned: false,
            };
            panic!("exercise emergency cleanup");
        });
        assert!(result.is_err());
        thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists());
    }
}
