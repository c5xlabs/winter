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
    cases.push(vec!["winter".into(), OsString::from_vec(vec![0xff, 0xfe])]);
    for args in cases {
        let result = reply(args);
        assert_eq!(result.exit_code, 2);
        assert!(result.stdout.is_empty());
        let text = String::from_utf8_lossy(&result.stderr);
        assert!(text.contains("--help") && !text.contains('\x1b'), "{text}");
    }
}
