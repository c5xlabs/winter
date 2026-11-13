//! Pinned development tools: `dev-tools.env` parsing and preflight.
//!
//! Tools are always invoked by absolute path under `<root>/.tools/bin`; `PATH` is never
//! consulted or modified. Preflight only probes versions. It never installs or downloads;
//! `scripts/bootstrap-dev.sh` is the one installer.

use super::{cargo_plain, run};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub const PIN_FILE: &str = "dev-tools.env";
const PROBE: Duration = Duration::from_secs(30);
const SETUP: &str = "run ./scripts/bootstrap-dev.sh (network required)";
// A closed loopback port: dprint can read its plugin cache but can never download.
const CLOSED_PROXY: &str = "http://127.0.0.1:1";

#[derive(Debug, PartialEq, Eq)]
pub struct Pins {
    pub dprint: String,
    pub rumdl: String,
    pub actionlint: String,
}

/// Mirrors `bootstrap-dev.sh`: only `^[A-Z_]+=\S+$` lines, exactly the three known keys,
/// no duplicates, no expansion.
pub fn parse_pins(text: &str) -> Result<Pins, String> {
    let (mut dprint, mut rumdl, mut actionlint) = (None, None, None);
    for line in text.lines() {
        let valid = line.split_once('=').is_some_and(|(k, v)| {
            !k.is_empty()
                && k.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
                && !v.is_empty()
                && !v.chars().any(char::is_whitespace)
        });
        if !valid {
            return Err(format!("{PIN_FILE}: invalid line: {line}"));
        }
        let (key, value) = line.split_once('=').expect("validated");
        let slot = match key {
            "DPRINT_VERSION" => &mut dprint,
            "RUMDL_VERSION" => &mut rumdl,
            "ACTIONLINT_VERSION" => &mut actionlint,
            _ => return Err(format!("{PIN_FILE}: unknown key: {line}")),
        };
        if slot.is_some() {
            return Err(format!("{PIN_FILE}: duplicate key: {line}"));
        }
        *slot = Some(value.to_owned());
    }
    let need = |v: Option<String>, k: &str| v.ok_or(format!("{PIN_FILE}: missing key: {k}"));
    Ok(Pins {
        dprint: need(dprint, "DPRINT_VERSION")?,
        rumdl: need(rumdl, "RUMDL_VERSION")?,
        actionlint: need(actionlint, "ACTIONLINT_VERSION")?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Dprint,
    Rumdl,
    Actionlint,
}
impl Tool {
    pub fn bin(self) -> &'static str {
        match self {
            Tool::Dprint => "dprint",
            Tool::Rumdl => "rumdl",
            Tool::Actionlint => "actionlint",
        }
    }
    fn pin(self, pins: &Pins) -> &str {
        match self {
            Tool::Dprint => &pins.dprint,
            Tool::Rumdl => &pins.rumdl,
            Tool::Actionlint => &pins.actionlint,
        }
    }
    fn probe(self) -> &'static str {
        match self {
            Tool::Actionlint => "-version",
            _ => "--version",
        }
    }
    // dprint and rumdl print `<name> <version>`; actionlint prints the bare version first.
    fn matches(self, output: &str, pin: &str) -> bool {
        let first = output.lines().next().unwrap_or("").trim();
        match self {
            Tool::Actionlint => first == pin,
            _ => first == format!("{} {pin}", self.bin()),
        }
    }
}

pub fn tool_path(root: &Path, tool: Tool) -> PathBuf {
    root.join(".tools/bin").join(tool.bin())
}

pub fn read_pins(root: &Path) -> Result<Pins, String> {
    let text = std::fs::read_to_string(root.join(PIN_FILE))
        .map_err(|e| format!("cannot read {PIN_FILE}: {e}"))?;
    parse_pins(&text)
}

/// Verifies one pinned tool by a bounded version probe and returns its absolute path.
pub fn preflight(root: &Path, tool: Tool, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let pins = read_pins(root)?;
    let pin = tool.pin(&pins);
    let path = tool_path(root, tool);
    let missing = |found: &str| {
        format!(
            "pinned tool {} {pin} is not available ({found}); {SETUP}",
            tool.bin()
        )
    };
    if !path.is_file() {
        return Err(missing(&format!("{} not found", path.display())));
    }
    let mut cmd = Command::new(&path);
    cmd.arg(tool.probe()).current_dir(root);
    // A tool being written by a concurrent installer or test can be briefly busy to exec.
    let mut probed = run(&mut cmd, cancel, PROBE);
    for _ in 0..5 {
        if !matches!(&probed, Err(e) if e.contains("Text file busy")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        probed = run(&mut cmd, cancel, PROBE);
    }
    let found = match probed {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(out) => return Err(missing(&format!("version probe failed: {}", out.status))),
        Err(e) => return Err(missing(&format!("version probe failed: {e}"))),
    };
    if tool.matches(&found, pin) {
        Ok(path)
    } else {
        Err(missing(&format!(
            "found `{}`",
            found.lines().next().unwrap_or("").trim()
        )))
    }
}

/// Verifies that the pinned toolchain provides a Cargo component (`fmt` or `clippy`).
pub fn preflight_component(
    root: &Path,
    component: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut cmd = cargo_plain(root);
    cmd.args([component, "--version"]);
    match run(&mut cmd, cancel, PROBE) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "Rust component {component} is not available ({}); {SETUP}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => Err(format!(
            "Rust component {component} is not available ({e}); {SETUP}"
        )),
    }
}

/// A dprint command that cannot download plugins.
pub fn dprint(root: &Path, tool: &Path, mode: &str) -> Command {
    let mut cmd = Command::new(tool);
    cmd.arg(mode).current_dir(root);
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        cmd.env(key, CLOSED_PROXY)
            .env(key.to_lowercase(), CLOSED_PROXY);
    }
    cmd.env_remove("NO_PROXY").env_remove("no_proxy");
    cmd
}

/// Adds setup guidance when dprint failed to resolve a plugin.
pub fn explain_dprint(result: Result<(), String>) -> Result<(), String> {
    result.map_err(|e| {
        if e.contains("Error resolving plugin") {
            format!("{e}\ndprint plugin cache is incomplete; {SETUP}")
        } else {
            e
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    pub const GOOD: &str =
        "DPRINT_VERSION=0.64.3\nRUMDL_VERSION=0.3.2\nACTIONLINT_VERSION=1.7.12\n";

    pub fn script(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn fixture() -> tempfile::TempDir {
        let dir = super::super::tests::scratch();
        fs::write(dir.path().join(PIN_FILE), GOOD).unwrap();
        dir
    }

    #[test]
    fn accepted_pin_file() {
        let pins = parse_pins(GOOD).unwrap();
        assert_eq!(pins.dprint, "0.64.3");
        assert_eq!(pins.rumdl, "0.3.2");
        assert_eq!(pins.actionlint, "1.7.12");
        // No trailing newline is accepted too.
        assert!(parse_pins(GOOD.trim_end()).is_ok());
    }

    #[test]
    fn rejected_pin_files() {
        for (text, needle) in [
            ("DPRINT_VERSION=1 2\n", "invalid line"),
            ("dprint_version=1\n", "invalid line"),
            ("DPRINT_VERSION=\n", "invalid line"),
            ("\nDPRINT_VERSION=1\n", "invalid line"),
            ("# comment\n", "invalid line"),
            ("OTHER=1\n", "unknown key"),
            ("DPRINT_VERSION=1\nDPRINT_VERSION=2\n", "duplicate key"),
            (
                "DPRINT_VERSION=1\nRUMDL_VERSION=1\n",
                "missing key: ACTIONLINT_VERSION",
            ),
        ] {
            let e = parse_pins(text).unwrap_err();
            assert!(e.contains(needle), "{text:?}: {e}");
        }
    }

    #[test]
    fn missing_tool_gives_setup_guidance() {
        let dir = fixture();
        let e = preflight(dir.path(), Tool::Rumdl, &AtomicBool::new(false)).unwrap_err();
        assert!(e.contains("rumdl 0.3.2") && e.contains("not found"), "{e}");
        assert!(e.contains("./scripts/bootstrap-dev.sh"), "{e}");
    }

    #[test]
    fn wrong_version_names_expected_and_found() {
        let dir = fixture();
        script(&tool_path(dir.path(), Tool::Rumdl), "echo 'rumdl 9.9.9'");
        let e = preflight(dir.path(), Tool::Rumdl, &AtomicBool::new(false)).unwrap_err();
        assert!(
            e.contains("0.3.2") && e.contains("found `rumdl 9.9.9`"),
            "{e}"
        );
        assert!(e.contains("./scripts/bootstrap-dev.sh"), "{e}");
    }

    #[test]
    fn probe_only_invokes_version_and_installs_nothing() {
        let dir = fixture();
        let marker = dir.path().join("marker");
        for (tool, ok, probe) in [
            (Tool::Dprint, "dprint 0.64.3", "--version"),
            (Tool::Rumdl, "rumdl 0.3.2", "--version"),
            (Tool::Actionlint, "1.7.12", "-version"),
        ] {
            script(
                &tool_path(dir.path(), tool),
                &format!(
                    "printf '%s\\n' \"$*\" >> '{}'\n[ \"$1\" = '{probe}' ] && echo '{ok}'",
                    marker.display()
                ),
            );
            let path = preflight(dir.path(), tool, &AtomicBool::new(false)).unwrap();
            assert_eq!(path, tool_path(dir.path(), tool));
        }
        assert_eq!(
            fs::read_to_string(&marker).unwrap(),
            "--version\n--version\n-version\n"
        );
        let mut files: Vec<_> = fs::read_dir(dir.path().join(".tools/bin"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        files.sort();
        assert_eq!(files, ["actionlint", "dprint", "rumdl"]);
    }

    #[test]
    fn dprint_command_cannot_download() {
        let cmd = dprint(Path::new("/"), Path::new("/x/dprint"), "check");
        let env: Vec<_> = cmd.get_envs().collect();
        for key in ["HTTP_PROXY", "https_proxy", "ALL_PROXY"] {
            assert!(env.iter().any(|(k, v)| *k == key && v.is_some()), "{key}");
        }
        for key in ["NO_PROXY", "no_proxy"] {
            assert!(env.iter().any(|(k, v)| *k == key && v.is_none()), "{key}");
        }
    }
}
