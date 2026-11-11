//! Development-only sample executable observed by the service harness.
//!
//! It deliberately depends on no workspace package and offers no arbitrary-command mode.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    process::ExitCode,
};

use serde::Serialize;

const PROBE_VARS: [&str; 2] = ["WINTER_SAMPLE_PROBE", "WINTER_DEV_SAMPLE_PATH"];

const USAGE: &str = "\
Usage:
  winter-dev-sample
  winter-dev-sample --help
  winter-dev-sample inspect [ARG...]
  winter-dev-sample copy-stdin
  winter-dev-sample emit
  winter-dev-sample exit CODE
  winter-dev-sample terminate
  winter-dev-sample wait
";

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Inspect(Vec<OsString>),
    Help,
    CopyStdin,
    Emit,
    Exit(u8),
    Terminate,
    Wait,
}

fn parse(args: &[OsString]) -> Option<Mode> {
    let Some(first) = args.first() else {
        return Some(Mode::Inspect(Vec::new()));
    };
    let rest = &args[1..];
    match first.to_str()? {
        "--help" if rest.is_empty() => Some(Mode::Help),
        "inspect" => Some(Mode::Inspect(args.to_vec())),
        "copy-stdin" if rest.is_empty() => Some(Mode::CopyStdin),
        "emit" if rest.is_empty() => Some(Mode::Emit),
        "terminate" if rest.is_empty() => Some(Mode::Terminate),
        "wait" if rest.is_empty() => Some(Mode::Wait),
        "exit" if rest.len() == 1 => parse_code(&rest[0]).map(Mode::Exit),
        _ => None,
    }
}

fn parse_code(value: &OsStr) -> Option<u8> {
    let text = value.to_str()?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Serialize)]
struct Inspection {
    pid: u32,
    argv0_hex: String,
    args_hex: Vec<String>,
    cwd_hex: String,
    env_hex: BTreeMap<&'static str, Option<String>>,
}

fn inspection(
    pid: u32,
    argv0: &OsStr,
    args: &[OsString],
    cwd: &[u8],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Inspection {
    Inspection {
        pid,
        argv0_hex: hex(argv0.as_bytes()),
        args_hex: args.iter().map(|a| hex(a.as_bytes())).collect(),
        cwd_hex: hex(cwd),
        env_hex: PROBE_VARS
            .iter()
            .map(|name| (*name, lookup(name).map(|v| hex(&v.into_vec()))))
            .collect(),
    }
}

fn inspect(argv0: &OsStr, args: &[OsString]) -> io::Result<()> {
    let cwd = std::env::current_dir()?;
    let record = inspection(
        std::process::id(),
        argv0,
        args,
        cwd.as_os_str().as_bytes(),
        |name| std::env::var_os(name),
    );
    let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut out = io::stdout().lock();
    out.write_all(&line)?;
    out.flush()
}

fn flush_ready(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// Reset the disposition and unblock `SIGINT`/`SIGTERM`, regardless of what the caller inherited.
fn reset_signals() {
    // SAFETY: plain libc signal-state calls on zero-initialised, locally owned structures.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for sig in [libc::SIGINT, libc::SIGTERM] {
            libc::signal(sig, libc::SIG_DFL);
            libc::sigaddset(&mut set, sig);
        }
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
    }
}

fn copy_stdin() -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().lock().read_to_end(&mut input)?;
    let mut out = io::stdout().lock();
    out.write_all(&input)?;
    out.flush()
}

fn usage_error() -> ExitCode {
    let _ = io::stderr().write_all(USAGE.as_bytes());
    ExitCode::from(2)
}

fn io_status(result: io::Result<()>) -> ExitCode {
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let mut raw = std::env::args_os();
    let argv0 = raw.next().unwrap_or_default();
    let args: Vec<OsString> = raw.collect();
    let Some(mode) = parse(&args) else {
        return usage_error();
    };
    match mode {
        Mode::Inspect(operands) => io_status(inspect(&argv0, &operands)),
        Mode::Help => io_status(flush_ready(USAGE)),
        Mode::CopyStdin => io_status(copy_stdin()),
        Mode::Emit => {
            let _ = io::stderr().write_all(b"sample stderr\n");
            io_status(flush_ready("sample stdout\n"))
        }
        Mode::Exit(code) => {
            let _ = flush_ready(&format!("sample exit {code}\n"));
            ExitCode::from(code)
        }
        Mode::Terminate => {
            reset_signals();
            if flush_ready("ready\n").is_err() {
                return ExitCode::FAILURE;
            }
            // SAFETY: raising a signal with default disposition terminates this process.
            unsafe { libc::raise(libc::SIGTERM) };
            ExitCode::FAILURE
        }
        Mode::Wait => {
            reset_signals();
            if flush_ready("ready\n").is_err() {
                return ExitCode::FAILURE;
            }
            loop {
                // SAFETY: pause has no preconditions; it returns only after a handled signal.
                unsafe { libc::pause() };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(items: &[&[u8]]) -> Vec<OsString> {
        items
            .iter()
            .map(|b| OsString::from_vec(b.to_vec()))
            .collect()
    }

    #[test]
    fn no_arguments_inspects() {
        assert_eq!(parse(&[]), Some(Mode::Inspect(vec![])));
    }

    #[test]
    fn inspect_operands_are_literal_including_delimiter() {
        let args = os(&[b"inspect", b"--", b"--help", b"\xff"]);
        assert_eq!(parse(&args), Some(Mode::Inspect(args.clone())));
    }

    #[test]
    fn initial_delimiter_and_unknown_modes_are_invalid() {
        assert_eq!(parse(&os(&[b"--", b"--help"])), None);
        assert_eq!(parse(&os(&[b"bogus"])), None);
        assert_eq!(parse(&os(&[b"\xff"])), None);
    }

    #[test]
    fn other_modes_reject_operands() {
        for mode in ["--help", "copy-stdin", "emit", "terminate", "wait"] {
            assert!(parse(&os(&[mode.as_bytes()])).is_some());
            assert_eq!(parse(&os(&[mode.as_bytes(), b"x"])), None, "{mode}");
        }
    }

    #[test]
    fn exit_requires_one_code_in_range() {
        assert_eq!(parse(&os(&[b"exit", b"0"])), Some(Mode::Exit(0)));
        assert_eq!(parse(&os(&[b"exit", b"255"])), Some(Mode::Exit(255)));
        for bad in [&b"256"[..], b"-1", b"+1", b"", b"a", b"\xff"] {
            assert_eq!(parse(&os(&[b"exit", bad])), None);
        }
        assert_eq!(parse(&os(&[b"exit"])), None);
        assert_eq!(parse(&os(&[b"exit", b"1", b"2"])), None);
    }

    #[test]
    fn inspection_preserves_bytes_and_nulls_absent_probes() {
        let args = os(&[b"inspect", b"\xff"]);
        let record = inspection(
            123,
            OsStr::new("/tmp/winter-dev-sample"),
            &args,
            b"/tmp",
            |name| (name == "WINTER_SAMPLE_PROBE").then(|| OsString::from("test")),
        );
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["pid"], 123);
        assert_eq!(
            json["argv0_hex"],
            "2f746d702f77696e7465722d6465762d73616d706c65"
        );
        assert_eq!(
            json["args_hex"],
            serde_json::json!(["696e7370656374", "ff"])
        );
        assert_eq!(json["cwd_hex"], "2f746d70");
        assert_eq!(json["env_hex"]["WINTER_SAMPLE_PROBE"], "74657374");
        assert!(json["env_hex"]["WINTER_DEV_SAMPLE_PATH"].is_null());
        assert_eq!(json["env_hex"].as_object().unwrap().len(), 2);
    }
}
