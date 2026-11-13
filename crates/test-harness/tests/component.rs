use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use winter_platform::{CliIdentity, Control, Reply, Request, control_reply, parse, usage_reply};

const ID: CliIdentity = CliIdentity {
    version: "7.6.5-component",
};

fn argv(args: &[&str]) -> Vec<OsString> {
    std::iter::once("not-winter")
        .chain(args.iter().copied())
        .map(Into::into)
        .collect()
}

fn reply(args: Vec<OsString>) -> Reply {
    match parse(args, &ID) {
        Ok(Request::Control(control)) => control_reply(control, ID),
        #[cfg(feature = "dev-sample")]
        Ok(Request::Sample(_)) => panic!("unexpected sample request"),
        Err(error) => usage_reply(error),
    }
}

#[test]
fn facade_help_and_identity() {
    let bare = reply(argv(&[]));
    for flag in ["-h", "--help"] {
        assert_eq!(reply(argv(&[flag])), bare);
    }
    assert_eq!(bare.exit_code, 0);
    assert!(
        bare.stderr.is_empty() && bare.stdout.ends_with(b"\n") && !bare.stdout.ends_with(b"\n\n")
    );
    let text = String::from_utf8(bare.stdout.clone()).unwrap();
    assert!(text.contains("Usage: winter") && !text.contains('\x1b'));
    assert!(text.lines().all(|line| line.chars().count() <= 100));
    assert_eq!(reply(vec![OsString::from_vec(vec![0xff])]), bare);
    for flag in ["-V", "--version"] {
        assert_eq!(
            parse(argv(&[flag]), &ID).unwrap(),
            Request::Control(Control::Version)
        );
        let version = reply(argv(&[flag]));
        assert_eq!(version.stdout, b"winter 7.6.5-component\n");
        assert!(version.stderr.is_empty() && version.exit_code == 0);
    }
}

#[test]
fn facade_unsupported_raw_input() {
    let mut cases = vec![argv(&["--bogus"]), argv(&["unknown"])];
    #[cfg(not(feature = "dev-sample"))]
    cases.push(argv(&["sample", "--help"]));
    cases.push(vec!["winter".into(), OsString::from_vec(vec![0xff, 0xfe])]);
    for args in cases {
        let result = reply(args);
        assert_eq!(result.exit_code, 2);
        assert!(result.stdout.is_empty());
        let text = String::from_utf8_lossy(&result.stderr);
        assert!(text.contains("--help") && !text.contains('\x1b'), "{text}");
    }
}

#[cfg(feature = "dev-sample")]
mod sample {
    use super::*;
    use std::convert::Infallible;
    use std::io;
    use winter_platform::{
        ExecRequest, LaunchError, Process, SAMPLE_PATH_ENV, launch_failure_reply, run_sample,
    };

    /// Records the request and fails like the OS does for an unrunnable file (`ENOEXEC`).
    #[derive(Default)]
    struct RecordingProcess {
        seen: Vec<ExecRequest>,
    }

    impl Process for RecordingProcess {
        fn replace(&mut self, request: ExecRequest) -> io::Result<Infallible> {
            self.seen.push(request);
            Err(io::Error::from_raw_os_error(libc::ENOEXEC))
        }
    }

    fn handoff(
        args: Vec<OsString>,
        path: Option<OsString>,
        process: &mut RecordingProcess,
    ) -> Result<Infallible, LaunchError> {
        let Request::Sample(request) = parse(args, &ID).unwrap() else {
            panic!("expected sample request");
        };
        run_sample(request, path, process)
    }

    #[test]
    fn absolute_path_and_raw_tail_reach_the_process_unchanged() {
        let path = OsString::from_vec(b"/opt/my sample/\xff".to_vec());
        let mut tail = argv(&["sample", "--help", "--", ""]);
        tail.push(OsString::from_vec(vec![0xff]));
        let mut process = RecordingProcess::default();
        let error = handoff(tail.clone(), Some(path.clone()), &mut process).unwrap_err();
        assert_eq!(process.seen.len(), 1);
        assert_eq!(process.seen[0].executable.as_os_str(), path);
        assert_eq!(process.seen[0].args, tail[2..].to_vec());
        let LaunchError::Replacement { source, .. } = &error else {
            panic!("expected replacement failure, got {error:?}");
        };
        assert_eq!(source.raw_os_error(), Some(libc::ENOEXEC));
        let reply = launch_failure_reply(error);
        assert_eq!(reply.exit_code, 1);
        assert!(reply.stdout.is_empty());
        let text = String::from_utf8_lossy(&reply.stderr);
        assert!(text.contains("development sample handoff failed"), "{text}");
        assert!(text.contains("/opt/my sample/"), "{text}");
        assert!(
            text.contains("os error 8") || text.contains("Exec format"),
            "{text}"
        );
    }

    #[test]
    fn invalid_paths_never_reach_the_process() {
        for (path, relative) in [
            (None, false),
            (Some(OsString::new()), false),
            (Some(OsString::from("rel/path")), true),
        ] {
            let mut process = RecordingProcess::default();
            let error = handoff(argv(&["sample"]), path, &mut process).unwrap_err();
            assert!(process.seen.is_empty());
            assert_eq!(matches!(error, LaunchError::RelativePath(_)), relative);
            let reply = launch_failure_reply(error);
            assert_eq!(reply.exit_code, 1);
            assert!(reply.stdout.is_empty());
            assert!(String::from_utf8_lossy(&reply.stderr).contains(SAMPLE_PATH_ENV));
        }
    }
}
