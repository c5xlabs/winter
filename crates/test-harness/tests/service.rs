use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
#[cfg(not(feature = "dev-sample"))]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use winter_test_harness::{Observation, ReplyStream, RuntimeFixture, Termination, invoke};

#[path = "service/scope_child.rs"]
mod scope_child;

fn process_field(pid: libc::pid_t, field: &str) -> Option<String> {
    let fixture = RuntimeFixture::new().ok()?;
    let mut call = fixture.invocation("/bin/ps");
    call.args = ["-o", field, "-p", &pid.to_string()]
        .into_iter()
        .map(Into::into)
        .collect();
    let output = invoke(call).ok()?;
    if output.termination != Termination::Code(0) {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

fn process_name_is(pid: libc::pid_t, name: &str) -> bool {
    process_field(pid, "comm=")
        .as_deref()
        .and_then(|value| std::path::Path::new(value).file_name())
        .is_some_and(|value| value == name)
}

struct Inputs {
    cli: PathBuf,
    version: String,
}

fn inputs() -> Inputs {
    let guidance = "service tests require driver inputs; run cargo dev test service --configuration production|development";
    let cli = PathBuf::from(std::env::var_os("WINTER_TEST_CLI_PATH").expect(guidance));
    assert!(
        cli.is_absolute() && cli.is_file(),
        "{guidance}: CLI path must be an absolute file"
    );
    let version = std::env::var("WINTER_TEST_EXPECTED_VERSION").expect(guidance);
    assert!(!version.is_empty(), "{guidance}: empty version");
    #[cfg(feature = "dev-sample")]
    {
        let sample = PathBuf::from(std::env::var_os("WINTER_TEST_SAMPLE_PATH").expect(guidance));
        assert!(
            sample.is_absolute() && sample.is_file(),
            "{guidance}: sample path must be an absolute file"
        );
    }
    #[cfg(not(feature = "dev-sample"))]
    assert!(
        std::env::var_os("WINTER_TEST_SAMPLE_PATH").is_none(),
        "{guidance}: production must not receive a sample artifact"
    );
    let driver: libc::pid_t = std::env::var("WINTER_TEST_PROCESS_SCOPE")
        .expect(guidance)
        .parse()
        .expect(guidance);
    let group = unsafe { libc::getpgrp() };
    let parent = unsafe { libc::getppid() };
    let driver_group = if driver > 0 {
        unsafe { libc::getpgid(driver) }
    } else {
        -1
    };
    // The driver starts Cargo as a process-group leader. Checking only that a
    // live PID is in another group accepts an unrelated process as the driver.
    // Ask the OS for the leader's parent and fail closed on any lookup error.
    let leader_parent = if group > 0 {
        process_field(group, "ppid=").and_then(|output| output.parse::<libc::pid_t>().ok())
    } else {
        None
    };
    assert!(
        group > 0
            && parent == group
            && driver_group > 0
            && driver_group != group
            && leader_parent == Some(driver)
            && process_name_is(driver, "winter-dev")
            && process_name_is(group, "cargo")
            && unsafe { libc::getpgid(parent) } == group,
        "{guidance}: invalid inherited driver process scope"
    );
    Inputs { cli, version }
}

#[test]
fn unrelated_live_process_is_not_a_driver_scope() {
    // First prove the outer invocation has the legitimate driver scope and
    // artifacts. The nested shell creates the same parent/group arrangement as
    // Cargo. Neither an unrelated live PID nor its actual non-driver parent
    // may authorize it.
    inputs();
    for claimed in [
        std::env::var("WINTER_TEST_PROCESS_SCOPE").unwrap(),
        std::process::id().to_string(),
    ] {
        let mut command = std::process::Command::new("/bin/sh");
        command
            .args([
                "-c",
                "\"$1\" --exact executable_help_and_version; exit $?",
                "scope-test",
            ])
            .arg(std::env::current_exe().unwrap())
            .env("WINTER_TEST_PROCESS_SCOPE", claimed);
        let output = scope_child::run(command).unwrap();
        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("invalid inherited driver process scope")
                || stderr.contains("invalid inherited driver process scope"),
            "stdout: {stdout}\nstderr: {stderr}"
        );
    }
}

fn run(fixture: &RuntimeFixture, cli: &PathBuf, args: Vec<OsString>) -> Observation {
    let mut call = fixture.invocation(cli);
    call.args = args;
    invoke(call).unwrap()
}

fn args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(Into::into).collect()
}

fn clean(fixture: &RuntimeFixture) {
    for name in ["cwd", "home", "config", "cache", "data", "state", "tmp"] {
        assert_eq!(
            fs::read_dir(fixture.path().join(name)).unwrap().count(),
            0,
            "{name} changed"
        );
    }
}

#[test]
fn executable_help_and_version() {
    let input = inputs();
    let fixture = RuntimeFixture::new().unwrap();
    let bare = run(&fixture, &input.cli, args(&[]));
    assert_eq!(bare.termination, Termination::Code(0));
    assert!(
        bare.stderr.is_empty() && bare.stdout.ends_with(b"\n") && !bare.stdout.ends_with(b"\n\n")
    );
    let text = String::from_utf8(bare.stdout.clone()).unwrap();
    assert!(text.contains("Usage: winter") && !text.contains('\x1b') && !text.contains('\r'));
    assert!(text.lines().all(|line| line.chars().count() <= 100));
    #[cfg(not(feature = "dev-sample"))]
    for forbidden in ["sample", "workspace", "hub", "runner"] {
        assert!(
            !text.contains(forbidden),
            "unexpected help entry: {forbidden}"
        );
    }
    for flag in ["-h", "--help"] {
        let result = run(&fixture, &input.cli, args(&[flag]));
        assert_eq!(result.termination, Termination::Code(0));
        assert_eq!(result.stdout, bare.stdout);
        assert!(result.stderr.is_empty());
    }
    for flag in ["-V", "--version"] {
        let result = run(&fixture, &input.cli, args(&[flag]));
        assert_eq!(result.termination, Termination::Code(0));
        assert_eq!(
            result.stdout,
            format!("winter {}\n", input.version).as_bytes()
        );
        assert!(result.stderr.is_empty());
    }
    clean(&fixture);
}

#[test]
fn executable_unknown_arguments() {
    let input = inputs();
    let fixture = RuntimeFixture::new().unwrap();
    let cases = [
        (args(&["--bogus"]), Some("--bogus")),
        (args(&["unknown"]), Some("unknown")),
        (vec![OsString::from_vec(vec![0xff, 0xfe])], None),
    ];
    for (case, token) in cases {
        let result = run(&fixture, &input.cli, case);
        assert_eq!(result.termination, Termination::Code(2));
        assert!(result.stdout.is_empty());
        let text = String::from_utf8_lossy(&result.stderr);
        assert!(text.contains("--help") && !text.contains('\x1b'), "{text}");
        if let Some(token) = token {
            assert!(text.contains(token), "{text}");
        }
    }
    clean(&fixture);
}

#[cfg(not(feature = "dev-sample"))]
#[test]
fn production_rejects_commands_and_decoys() {
    let input = inputs();
    let fixture = RuntimeFixture::new().unwrap();
    let marker = fixture.path().join("cwd/decoy-ran");
    for name in ["winter", "sample", "winter-dev-sample"] {
        let path = fixture.path().join("bin").join(name);
        fs::write(&path, b"#!/bin/sh\nprintf ran > \"$MARKER\"\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for name in ["winter", "sample", "winter-dev-sample"] {
        let mut probe = fixture.invocation("/bin/sh");
        probe.args = vec![
            "-c".into(),
            "exec \"$1\"".into(),
            "probe".into(),
            name.into(),
        ];
        probe
            .env
            .insert("MARKER".into(), marker.clone().into_os_string());
        assert_eq!(invoke(probe).unwrap().termination, Termination::Code(0));
        assert_eq!(fs::read(&marker).unwrap(), b"ran");
        fs::remove_file(&marker).unwrap();
    }
    for mode in 0..3 {
        for case in [
            args(&["workspace"]),
            args(&["hub"]),
            args(&["runner"]),
            args(&["sample"]),
            args(&["sample", "--help"]),
        ] {
            let mut call = fixture.invocation(&input.cli);
            call.args = case.clone();
            call.env
                .insert("MARKER".into(), marker.clone().into_os_string());
            if mode != 1 {
                call.env.insert(
                    "WINTER_DEV_SAMPLE_PATH".into(),
                    fixture
                        .path()
                        .join("bin/winter-dev-sample")
                        .into_os_string(),
                );
            }
            if mode == 0 {
                call.env.insert(
                    "PATH".into(),
                    fixture.path().join("empty-path").into_os_string(),
                );
            }
            let result = invoke(call).unwrap();
            assert_eq!(result.termination, Termination::Code(2));
            assert!(result.stdout.is_empty());
            let text = String::from_utf8_lossy(&result.stderr);
            assert!(
                text.contains(case[0].to_str().unwrap()) && text.contains("--help"),
                "{text}"
            );
            assert!(!marker.exists());
        }
    }
    clean(&fixture);
}

#[test]
fn executable_reply_write_failures() {
    let input = inputs();
    let fixture = RuntimeFixture::new().unwrap();
    for flag in ["--help", "--version"] {
        let mut call = fixture.invocation(&input.cli);
        call.args = args(&[flag]);
        call.reject_reply_writes = Some(ReplyStream::Stdout);
        let result = invoke(call).unwrap();
        assert_eq!(result.termination, Termination::Code(1));
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("failed to write reply"));
    }
    let mut call = fixture.invocation(&input.cli);
    call.args = args(&["--bogus"]);
    call.reject_reply_writes = Some(ReplyStream::Stderr);
    let result = invoke(call).unwrap();
    assert_eq!(result.termination, Termination::Code(1));
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
    clean(&fixture);
}
