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
    #[cfg(feature = "dev-sample")]
    sample: PathBuf,
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
    let sample = PathBuf::from(std::env::var_os("WINTER_TEST_SAMPLE_PATH").expect(guidance));
    #[cfg(feature = "dev-sample")]
    assert!(
        sample.is_absolute() && sample.is_file(),
        "{guidance}: sample path must be an absolute file"
    );
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
    Inputs {
        cli,
        version,
        #[cfg(feature = "dev-sample")]
        sample,
    }
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
    #[cfg(feature = "dev-sample")]
    assert!(text.contains("winter sample [ARG...]"), "{text}");
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

#[cfg(feature = "dev-sample")]
mod sample {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    use winter_test_harness::{Completion, FailureKind, TestSignal};

    const PATH_VAR: &str = "WINTER_DEV_SAMPLE_PATH";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn os(bytes: &[u8]) -> OsString {
        OsString::from_vec(bytes.to_vec())
    }

    /// An invocation of `winter sample ARGS` with the sample path set.
    fn call(
        fixture: &RuntimeFixture,
        input: &Inputs,
        tail: Vec<OsString>,
    ) -> winter_test_harness::Invocation {
        let mut call = fixture.invocation(&input.cli);
        call.args = std::iter::once(OsString::from("sample"))
            .chain(tail)
            .collect();
        call.env
            .insert(PATH_VAR.into(), input.sample.clone().into_os_string());
        call
    }

    fn json(observation: &Observation) -> serde_json::Value {
        serde_json::from_slice(&observation.stdout).unwrap()
    }

    fn executable(path: &std::path::Path, body: &[u8]) {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn replacement_preserves_pid_argv_cwd_and_environment() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let mut c = call(
            &fixture,
            &input,
            vec![
                "inspect".into(),
                "--help".into(),
                "".into(),
                "a b".into(),
                "-V".into(),
                "--".into(),
                os(b"\xff"),
            ],
        );
        c.env.insert("WINTER_SAMPLE_PROBE".into(), "probe".into());
        let result = invoke(c).unwrap();
        assert_eq!(result.termination, Termination::Code(0));
        let v = json(&result);
        assert_eq!(v["pid"], result.spawned_pid);
        assert_eq!(v["argv0_hex"], hex(input.sample.as_os_str().as_bytes()));
        let want: Vec<String> = [
            &b"inspect"[..],
            b"--help",
            b"",
            b"a b",
            b"-V",
            b"--",
            b"\xff",
        ]
        .iter()
        .map(|b| hex(b))
        .collect();
        assert_eq!(v["args_hex"], serde_json::json!(want));
        assert_eq!(
            v["cwd_hex"],
            hex(fixture.path().join("cwd").as_os_str().as_bytes())
        );
        assert_eq!(v["env_hex"]["WINTER_SAMPLE_PROBE"], hex(b"probe"));
        assert!(v["env_hex"][PATH_VAR].is_null());
        assert!(result.stderr.is_empty());
        clean(&fixture);
    }

    #[test]
    fn environment_is_preserved_except_the_exact_sample_key() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let mut c = call(&fixture, &input, vec![]);
        c.executable = input.cli.clone();
        c.env.insert(PATH_VAR.into(), "/usr/bin/env".into());
        c.args = vec!["sample".into(), "-0".into()];
        c.env
            .insert("WINTER_DEV_SAMPLE_PATH_EXTRA".into(), "decoy".into());
        c.env.insert("lower".into(), os(b"\xff\xfe"));
        let mut expected: Vec<Vec<u8>> = c
            .env
            .iter()
            .filter(|(k, _)| k.as_bytes() != PATH_VAR.as_bytes())
            .map(|(k, v)| [k.as_bytes(), b"=", v.as_bytes()].concat())
            .collect();
        let result = invoke(c).unwrap();
        assert_eq!(result.termination, Termination::Code(0));
        let mut actual: Vec<Vec<u8>> = result
            .stdout
            .split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    fn non_utf8_spaced_executable_path_is_argv0() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let mut name = b"sample ".to_vec();
        name.push(0xff);
        let copy = fixture.path().join("bin").join(OsString::from_vec(name));
        fs::copy(&input.sample, &copy).unwrap();
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o700)).unwrap();
        let mut c = call(&fixture, &input, vec!["inspect".into()]);
        c.env.insert(PATH_VAR.into(), copy.clone().into_os_string());
        let result = invoke(c).unwrap();
        assert_eq!(json(&result)["argv0_hex"], hex(copy.as_os_str().as_bytes()));
    }

    #[test]
    fn streams_stdin_and_exit_status_pass_through() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let mut c = call(&fixture, &input, vec!["copy-stdin".into()]);
        c.stdin = vec![0xff, 0x00, b'\n', 0xfe];
        let r = invoke(c).unwrap();
        assert_eq!(r.termination, Termination::Code(0));
        assert_eq!(r.stdout, [0xff, 0x00, b'\n', 0xfe]);

        let r = invoke(call(&fixture, &input, vec!["emit".into()])).unwrap();
        assert_eq!(r.stdout, b"sample stdout\n");
        assert_eq!(r.stderr, b"sample stderr\n");

        let r = invoke(call(&fixture, &input, vec!["--help".into()])).unwrap();
        assert_eq!(r.termination, Termination::Code(0));
        assert!(String::from_utf8_lossy(&r.stdout).contains("winter-dev-sample"));

        let r = invoke(call(&fixture, &input, vec!["--".into(), "--help".into()])).unwrap();
        assert_eq!(r.termination, Termination::Code(2));
        assert!(String::from_utf8_lossy(&r.stderr).contains("winter-dev-sample"));

        for code in [0, 1, 37, 255] {
            let r = invoke(call(
                &fixture,
                &input,
                vec!["exit".into(), code.to_string().into()],
            ))
            .unwrap();
            assert_eq!(r.termination, Termination::Code(code));
        }
        let r = invoke(call(&fixture, &input, vec!["terminate".into()])).unwrap();
        assert_eq!(r.termination, Termination::Signal(libc::SIGTERM));
        for (signal, number) in [
            (TestSignal::Interrupt, libc::SIGINT),
            (TestSignal::Terminate, libc::SIGTERM),
        ] {
            let mut c = call(&fixture, &input, vec!["wait".into()]);
            c.completion = Completion::SignalAfterStdout {
                marker: b"ready\n".to_vec(),
                signal,
            };
            let r = invoke(c).unwrap();
            assert_eq!(r.termination, Termination::Signal(number));
        }
        clean(&fixture);
    }

    /// Runs `winter sample` with the given path value (or unset) and asserts the launcher failure.
    fn assert_launch_failure(fixture: &RuntimeFixture, input: &Inputs, path: Option<OsString>) {
        let mut c = call(fixture, input, vec![]);
        c.env.remove(OsString::from(PATH_VAR).as_os_str());
        if let Some(path) = path {
            c.env.insert(PATH_VAR.into(), path);
        }
        let r = invoke(c).unwrap();
        assert_eq!(r.termination, Termination::Code(1));
        assert!(r.stdout.is_empty());
        let text = String::from_utf8_lossy(&r.stderr);
        assert!(text.contains("development sample handoff failed"), "{text}");
    }

    #[test]
    fn invalid_targets_fail_without_shell_or_path_fallback() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let cwd = fixture.path().join("cwd");
        let marker = fixture.path().join("tmp/marker");
        let body = format!("printf ran > '{}'\n", marker.display());
        executable(
            &fixture.path().join("bin/winter-dev-sample"),
            format!("#!/bin/sh\n{body}").as_bytes(),
        );
        let dir = fixture.path().join("bin");
        let zero_six = fixture.path().join("tmp/mode0600");
        fs::write(&zero_six, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&zero_six, fs::Permissions::from_mode(0o600)).unwrap();
        let garbage = fixture.path().join("tmp/garbage");
        executable(
            &garbage,
            &[0x7f, b'X', 0xff, 0x01, 0x02, 0x03, 0xde, 0xad, 0xbe, 0xef],
        );
        let noshebang = fixture.path().join("tmp/noshebang");
        executable(&noshebang, body.as_bytes());

        for path in [
            None,
            Some(OsString::new()),
            Some("winter-dev-sample".into()),
            Some(fixture.path().join("tmp/absent").into_os_string()),
            Some(dir.clone().into_os_string()),
            Some(zero_six.into_os_string()),
            Some(garbage.into_os_string()),
            Some(noshebang.into_os_string()),
        ] {
            assert_launch_failure(&fixture, &input, path);
            // The launcher has terminated: a marker now would prove a fallback ran.
            assert!(!marker.exists(), "fallback executed a script");
        }
        // Positive control: the same body with a kernel interpreter line does run.
        let shebang = fixture.path().join("tmp/shebang");
        executable(&shebang, format!("#!/bin/sh\n{body}").as_bytes());
        let mut c = call(&fixture, &input, vec![]);
        c.env.insert(PATH_VAR.into(), shebang.into_os_string());
        assert_eq!(invoke(c).unwrap().termination, Termination::Code(0));
        assert_eq!(fs::read(&marker).unwrap(), b"ran");
        let _ = cwd;
    }

    #[test]
    fn failed_replacement_differs_from_failing_sample() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let sample = invoke(call(&fixture, &input, vec!["exit".into(), "1".into()])).unwrap();
        assert_eq!(sample.termination, Termination::Code(1));
        assert_eq!(sample.stdout, b"sample exit 1\n");
        assert!(sample.stderr.is_empty());
        assert_launch_failure(
            &fixture,
            &input,
            Some(fixture.path().join("none").into_os_string()),
        );
    }

    #[test]
    fn timeout_leaves_no_owned_fixture() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let mut c = call(&fixture, &input, vec!["wait".into()]);
        c.timeout = Duration::from_millis(500);
        let failure = invoke(c).unwrap_err();
        assert!(
            matches!(failure.kind, FailureKind::Timeout),
            "{:?}",
            failure.kind
        );
        let pid = failure.spawned_pid.unwrap() as libc::pid_t;
        // The harness guard owns the (replaced) child directly, so it is gone.
        let gone = unsafe { libc::kill(pid, 0) };
        assert_eq!(gone, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sigpipe_is_not_ignored_in_the_sample() {
        let input = inputs();
        let fixture = RuntimeFixture::new().unwrap();
        let probe = |c: winter_test_harness::Invocation| -> u64 {
            let r = invoke(c).unwrap();
            let text = String::from_utf8(r.stdout).unwrap();
            let line = text.lines().find(|l| l.starts_with("SigIgn:")).unwrap();
            u64::from_str_radix(line.split_whitespace().nth(1).unwrap(), 16).unwrap()
        };
        let script: Vec<OsString> = vec!["-c".into(), "/bin/cat /proc/$$/status".into()];
        let mut via = call(&fixture, &input, script.clone());
        via.env.insert(PATH_VAR.into(), "/bin/sh".into());
        let bit = 1u64 << (libc::SIGPIPE - 1);
        // The launcher's Rust runtime ignores SIGPIPE; the handoff must not leak that.
        assert_eq!(probe(via) & bit, 0, "sample inherited an ignored SIGPIPE");
    }
}
