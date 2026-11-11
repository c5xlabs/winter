use std::ffi::OsString;

use clap::Command;
use clap::error::ErrorKind;

/// Identity of the executable hosting the platform.
#[derive(Clone, Copy, Debug)]
pub struct CliIdentity {
    pub version: &'static str,
}

/// Control requests answered without side effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Help,
    Version,
}

/// A parsed command-line request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Control(Control),
}

/// Bytes to write to the standard streams and the exit code to return.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: u8,
}

/// A command-line usage error carrying clap's rendered diagnostic.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(String);

fn command(identity: &CliIdentity) -> Command {
    Command::new("winter")
        .bin_name("winter")
        .about("Winter platform command")
        .version(identity.version)
        .disable_help_subcommand(true)
        .no_binary_name(true)
}

/// Parses `argv` (including `argv[0]`, which is discarded undecoded).
pub fn parse(
    argv: impl IntoIterator<Item = OsString>,
    identity: &CliIdentity,
) -> Result<Request, UsageError> {
    let rest = argv.into_iter().skip(1);
    match command(identity).try_get_matches_from(rest) {
        Ok(_) => Ok(Request::Control(Control::Help)),
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp => Ok(Request::Control(Control::Help)),
            ErrorKind::DisplayVersion => Ok(Request::Control(Control::Version)),
            _ => Err(UsageError(err.render().to_string())),
        },
    }
}

/// Renders the reply for a control request.
pub fn control_reply(request: Control, identity: CliIdentity) -> Reply {
    let stdout = match request {
        Control::Help => {
            let mut text = command(&identity).render_help().to_string();
            text.truncate(text.trim_end_matches('\n').len());
            text.push('\n');
            text
        }
        Control::Version => command(&identity).render_version(),
    };
    Reply {
        stdout: stdout.into_bytes(),
        stderr: Vec::new(),
        exit_code: 0,
    }
}

/// Renders the reply for a usage error.
pub fn usage_reply(error: UsageError) -> Reply {
    Reply {
        stdout: Vec::new(),
        stderr: error.0.into_bytes(),
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    const ID: CliIdentity = CliIdentity {
        version: "9.8.7-test",
    };

    fn argv(args: &[&str]) -> Vec<OsString> {
        std::iter::once("winter")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    fn help_for(argv: Vec<OsString>) -> Reply {
        match parse(argv, &ID).unwrap() {
            Request::Control(c) => control_reply(c, ID),
        }
    }

    #[test]
    fn bare_short_and_long_help_are_identical() {
        let bare = help_for(argv(&[]));
        for args in [&["-h"][..], &["--help"][..], &["--"][..]] {
            assert_eq!(
                parse(argv(args), &ID).unwrap(),
                Request::Control(Control::Help)
            );
            assert_eq!(help_for(argv(args)), bare);
        }
        assert!(bare.stdout.ends_with(b"\n") && !bare.stdout.ends_with(b"\n\n"));
        assert!(bare.stderr.is_empty());
        assert_eq!(bare.exit_code, 0);
    }

    #[test]
    fn help_content_is_production_only() {
        let text = String::from_utf8(help_for(argv(&[])).stdout).unwrap();
        assert!(text.contains("Winter platform"));
        for want in ["Usage: winter", "-h, --help", "-V, --version"] {
            assert!(text.contains(want), "missing {want}");
        }
        for banned in ["sample", "workspace", "hub", "runner", "install", "\x1b"] {
            assert!(!text.contains(banned), "unexpected {banned}");
        }
        assert!(text.lines().all(|l| l.chars().count() <= 100));
    }

    #[test]
    fn version_uses_injected_identity() {
        for flag in ["-V", "--version"] {
            let Request::Control(c) = parse(argv(&[flag]), &ID).unwrap();
            assert_eq!(c, Control::Version);
            let reply = control_reply(c, ID);
            assert_eq!(reply.stdout, b"winter 9.8.7-test\n");
            assert!(reply.stderr.is_empty());
            assert_eq!(reply.exit_code, 0);
        }
        assert_ne!(ID.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn argv0_is_ignored() {
        let expected = help_for(argv(&[]));
        let elsewhere = vec![OsString::from("/elsewhere/not-winter")];
        let non_utf8 = vec![OsString::from_vec(vec![0xff, 0xfe])];
        assert_eq!(help_for(elsewhere), expected);
        assert_eq!(help_for(non_utf8), expected);
        assert!(
            String::from_utf8(expected.stdout)
                .unwrap()
                .contains("Usage: winter")
        );
    }

    #[test]
    fn unsupported_input_is_a_usage_error() {
        let cases: [(&[&str], Option<&str>); 6] = [
            (&["--bogus"], Some("--bogus")),
            (&["workspace"], Some("workspace")),
            (&["hub"], Some("hub")),
            (&["runner"], Some("runner")),
            (&["sample"], Some("sample")),
            (&["sample", "--help"], Some("sample")),
        ];
        for (args, token) in cases {
            let err = parse(argv(args), &ID).unwrap_err();
            let reply = usage_reply(err);
            let stderr = String::from_utf8(reply.stderr).unwrap();
            assert!(reply.stdout.is_empty());
            assert_eq!(reply.exit_code, 2);
            assert!(stderr.contains(token.unwrap()), "{stderr}");
            assert!(
                stderr.contains("--help") && !stderr.contains('\x1b'),
                "{stderr}"
            );
        }
        let mut bad = argv(&[]);
        bad.push(OsString::from_vec(vec![0xff, 0xfe]));
        let reply = usage_reply(parse(bad, &ID).unwrap_err());
        assert!(reply.stdout.is_empty() && reply.exit_code == 2);
        assert!(String::from_utf8_lossy(&reply.stderr).contains("--help"));
    }
}
