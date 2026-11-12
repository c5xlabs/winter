use serde_json::Value;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
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
        while !self.reaped {
            if self.child.try_wait()?.is_some() {
                self.reaped = true;
                break;
            }
            if Instant::now() >= until {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child did not reap",
                ));
            }
            thread::sleep(POLL);
        }
        self.cleaned = true;
        Ok(())
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            eprintln!("winter-dev: emergency cleanup: {e}");
        }
    }
}
struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
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
    let stdout = fs::read(out).map_err(|e| format!("reading child stdout: {e}"))?;
    let stderr = fs::read(err).map_err(|e| format!("reading child stderr: {e}"))?;
    if let Err(e) = cleanup {
        return Err(format!(
            "cleanup failed: {e}\n{}\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        ));
    }
    match result {
        Ok(status) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        Err(e) => Err(format!(
            "{e}\n{}\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        )),
    }
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
    serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid Cargo metadata: {e}"))
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
fn artifact(bytes: &[u8], id: &str, name: &str, root: &Path) -> Result<PathBuf, String> {
    let mut found = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let msg: Value =
            serde_json::from_slice(line).map_err(|e| format!("invalid Cargo JSON: {e}"))?;
        if msg["reason"] == "compiler-message"
            && let Some(rendered) = msg["message"]["rendered"].as_str()
        {
            eprint!("{rendered}");
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
            found.push(if path.is_absolute() {
                path
            } else {
                root.join(path)
            });
        }
    }
    if found.len() != 1 {
        return Err(format!(
            "expected exactly one executable artifact for {name}, found {}",
            found.len()
        ));
    }
    Ok(found.remove(0))
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
    artifact(&output.stdout, id, bin, root)
}
struct Artifacts {
    cli: PathBuf,
    sample: Option<PathBuf>,
    version: String,
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
            let mut cmd = Command::new(a.cli);
            cmd.args(args)
                .env("WINTER_DEV_SAMPLE_PATH", a.sample.ok_or("missing sample")?);
            Err(format!("exec failed: {}", cmd.exec()))
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
    use std::sync::atomic::AtomicBool;

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
        assert!(artifact(b"not json", "exact", "winter", Path::new("/")).is_err());
        assert!(artifact(br#"{"reason":"compiler-artifact","package_id":"exact","target":{"name":"winter","kind":["bin"]},"executable":null}"#, "exact", "winter", Path::new("/")).is_err());
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
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("(/bin/sleep 0.3; printf bad > \"$1\") & exit 0")
            .arg("sh")
            .arg(&marker);
        assert!(
            run(&mut cmd, &AtomicBool::new(false), Duration::from_secs(3))
                .unwrap()
                .status
                .success()
        );
        thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists());
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
