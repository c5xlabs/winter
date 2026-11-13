//! The named verification operations and the runner shared by `check`, `format`, `lint`
//! and `architecture`. One ordered list owns every operation; each verb runs its slice.

use super::tools::{self, Tool};
use super::{
    Configuration, Tier, build, cargo, cargo_plain, checked, features, metadata_bytes, test,
};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use winter_test_harness::architecture;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slice {
    Format,
    Lint,
    Architecture,
    Build,
    Test,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    RustfmtCheck,
    DprintCheck,
    RustfmtWrite,
    DprintWrite,
    Clippy(Configuration),
    Rumdl,
    Actionlint,
    BashSyntax(&'static str),
    ArchitectureSelfTest,
    ArchitectureAllowlist,
    Build(Configuration),
    Test(Tier, Configuration),
}

const BOTH: [Configuration; 2] = [Configuration::Production, Configuration::Development];
const TIERS: [Tier; 3] = [Tier::Unit, Tier::Component, Tier::Service];
const SCRIPTS: [&str; 2] = ["scripts/bootstrap-dev.sh", "scripts/winter-cli"];

fn config_name(c: Configuration) -> &'static str {
    match c {
        Configuration::Production => "production",
        Configuration::Development => "development",
    }
}

impl Op {
    pub fn name(self) -> String {
        match self {
            Op::RustfmtCheck => "rustfmt-check".into(),
            Op::DprintCheck => "dprint-check".into(),
            Op::RustfmtWrite => "rustfmt-write".into(),
            Op::DprintWrite => "dprint-write".into(),
            Op::Clippy(c) => format!("clippy-{}", config_name(c)),
            Op::Rumdl => "rumdl".into(),
            Op::Actionlint => "actionlint".into(),
            Op::BashSyntax(script) => format!(
                "bash-syntax-{}",
                Path::new(script)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(script)
                    .trim_end_matches("-dev")
            ),
            Op::ArchitectureSelfTest => "architecture-self-test".into(),
            Op::ArchitectureAllowlist => "architecture-allowlist".into(),
            Op::Build(c) => format!("build-{}", config_name(c)),
            Op::Test(t, c) => format!(
                "test-{}-{}",
                match t {
                    Tier::Unit => "unit",
                    Tier::Component => "component",
                    Tier::Service => "service",
                },
                config_name(c)
            ),
        }
    }
}

pub fn slice(which: Slice) -> Vec<Op> {
    match which {
        Slice::Format => vec![Op::RustfmtCheck, Op::DprintCheck],
        Slice::Lint => BOTH
            .into_iter()
            .map(Op::Clippy)
            .chain([Op::Rumdl, Op::Actionlint])
            .chain(SCRIPTS.into_iter().map(Op::BashSyntax))
            .collect(),
        Slice::Architecture => vec![Op::ArchitectureSelfTest, Op::ArchitectureAllowlist],
        Slice::Build => BOTH.into_iter().map(Op::Build).collect(),
        Slice::Test => TIERS
            .into_iter()
            .flat_map(|t| BOTH.into_iter().map(move |c| Op::Test(t, c)))
            .collect(),
    }
}

/// The one ordered list `cargo dev check` runs.
pub fn all() -> Vec<Op> {
    [
        Slice::Format,
        Slice::Lint,
        Slice::Architecture,
        Slice::Build,
        Slice::Test,
    ]
    .into_iter()
    .flat_map(slice)
    .collect()
}

pub fn format_write() -> Vec<Op> {
    vec![Op::RustfmtWrite, Op::DprintWrite]
}

/// Runs every operation, continuing after failures so one run reports them all. Stops at
/// cancellation.
pub fn run_ops(
    ops: &[Op],
    cancel: &AtomicBool,
    mut exec: impl FnMut(Op) -> Result<(), String>,
) -> Result<(), String> {
    let total = ops.len();
    let mut failed = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let name = op.name();
        eprintln!("winter-dev: [{}/{total}] {name}", i + 1);
        match exec(*op) {
            Ok(()) => eprintln!("winter-dev: [{}/{total}] {name} ok", i + 1),
            Err(e) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                eprintln!("{e}");
                eprintln!("winter-dev: [{}/{total}] {name} FAILED", i + 1);
                failed.push(name);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("failed checks: {}", failed.join(", ")))
    }
}

pub fn execute(root: &Path, op: Op, cancel: &AtomicBool) -> Result<(), String> {
    match op {
        Op::RustfmtCheck => rustfmt(root, false, cancel),
        Op::RustfmtWrite => rustfmt(root, true, cancel),
        Op::DprintCheck => dprint(root, "check", cancel),
        Op::DprintWrite => dprint(root, "fmt", cancel),
        Op::Clippy(c) => clippy(root, c, cancel),
        Op::Rumdl => {
            let tool = tools::preflight(root, Tool::Rumdl, cancel)?;
            let mut cmd = Command::new(tool);
            cmd.args(["check", "."]).current_dir(root);
            checked(&mut cmd, cancel).map(|_| ())
        }
        Op::Actionlint => actionlint(root, cancel),
        Op::BashSyntax(script) => bash_syntax(root, script, cancel),
        Op::ArchitectureSelfTest => architecture_self_test(root, cancel),
        Op::ArchitectureAllowlist => architecture_allowlist(root, cancel),
        Op::Build(c) => build(root, c, cancel).map(|_| ()),
        Op::Test(t, c) => test(root, t, c, None, cancel),
    }
}

pub fn rustfmt(root: &Path, write: bool, cancel: &AtomicBool) -> Result<(), String> {
    tools::preflight_component(root, "fmt", cancel)?;
    let mut cmd = cargo_plain(root);
    cmd.args(["fmt", "--all"]);
    if !write {
        cmd.args(["--", "--check"]);
    }
    checked(&mut cmd, cancel).map(|_| ())
}

pub fn dprint(root: &Path, mode: &str, cancel: &AtomicBool) -> Result<(), String> {
    let tool = tools::preflight(root, Tool::Dprint, cancel)?;
    let mut cmd = tools::dprint(root, &tool, mode);
    tools::explain_dprint(checked(&mut cmd, cancel).map(|_| ()))
}

const CLIPPY_PACKAGES: [&str; 3] = ["winter-platform", "winter-cli", "winter-test-harness"];

fn clippy(root: &Path, config: Configuration, cancel: &AtomicBool) -> Result<(), String> {
    tools::preflight_component(root, "clippy", cancel)?;
    let mut packages = CLIPPY_PACKAGES.to_vec();
    if config == Configuration::Development {
        packages.push("winter-dev-sample");
    }
    for pkg in packages {
        let mut cmd = cargo(root, config, "clippy");
        cmd.args(["-p", pkg, "--all-targets"]);
        features(&mut cmd, config, pkg);
        checked(&mut cmd, cancel)?;
    }
    Ok(())
}

/// Shellcheck and pyflakes integrations are disabled so a host's unrelated tools cannot
/// change the result.
pub fn actionlint(root: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let tool = tools::preflight(root, Tool::Actionlint, cancel)?;
    checked(&mut actionlint_command(root, &tool), cancel).map(|_| ())
}
fn actionlint_command(root: &Path, tool: &Path) -> Command {
    let mut cmd = Command::new(tool);
    cmd.args(["-shellcheck=", "-pyflakes="]).current_dir(root);
    cmd
}

pub fn bash_syntax(root: &Path, script: &str, cancel: &AtomicBool) -> Result<(), String> {
    let mut cmd = Command::new("bash");
    cmd.args(["-n", script]).current_dir(root);
    checked(&mut cmd, cancel).map(|_| ())
}

/// The number of tests a libtest summary reports as passed, if it reports success.
pub fn passed_tests(stdout: &str) -> Option<u64> {
    stdout.lines().find_map(|l| {
        l.strip_prefix("test result: ok. ")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn architecture_self_test(root: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let mut cmd = cargo(root, Configuration::Production, "test");
    cmd.args(["-p", "winter-test-harness", "--lib", "architecture::"]);
    let out = checked(&mut cmd, cancel)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    print!("{stdout}");
    match passed_tests(&stdout) {
        Some(n) if n > 0 => Ok(()),
        _ => Err("architecture self-tests ran no passing tests".into()),
    }
}

fn architecture_allowlist(root: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let data = metadata_bytes(root, Configuration::Production, cancel)?;
    let meta: cargo_metadata::Metadata =
        serde_json::from_slice(&data).map_err(|e| format!("invalid Cargo metadata: {e}"))?;
    let mut problems = Vec::new();
    if let Err(e) = architecture::check_workspace_members(&meta) {
        problems.push(e.to_string());
    }
    if let Err(violations) = architecture::check_workspace_boundaries(&meta) {
        problems.extend(violations.iter().map(ToString::to_string));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_is_the_concatenation_of_the_slices() {
        let mut expected = Vec::new();
        for s in [
            Slice::Format,
            Slice::Lint,
            Slice::Architecture,
            Slice::Build,
            Slice::Test,
        ] {
            expected.extend(slice(s));
        }
        assert_eq!(all(), expected);
    }

    #[test]
    fn names_are_the_documented_ordered_list() {
        let names: Vec<_> = all().into_iter().map(Op::name).collect();
        assert_eq!(
            names,
            [
                "rustfmt-check",
                "dprint-check",
                "clippy-production",
                "clippy-development",
                "rumdl",
                "actionlint",
                "bash-syntax-bootstrap",
                "bash-syntax-winter-cli",
                "architecture-self-test",
                "architecture-allowlist",
                "build-production",
                "build-development",
                "test-unit-production",
                "test-unit-development",
                "test-component-production",
                "test-component-development",
                "test-service-production",
                "test-service-development",
            ]
        );
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
    }

    #[test]
    fn every_configuration_and_tier_is_present() {
        let ops = all();
        for c in BOTH {
            assert!(ops.contains(&Op::Clippy(c)) && ops.contains(&Op::Build(c)));
            for t in TIERS {
                assert!(ops.contains(&Op::Test(t, c)));
            }
        }
    }

    #[test]
    fn a_failing_operation_fails_the_aggregate_and_later_operations_still_run() {
        let ops = all();
        let mut ran = Vec::new();
        let result = run_ops(&ops, &AtomicBool::new(false), |op| {
            ran.push(op);
            if matches!(op, Op::Rumdl | Op::Build(Configuration::Development)) {
                Err("diagnostic".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(ran, ops);
        assert_eq!(
            result.unwrap_err(),
            "failed checks: rumdl, build-development"
        );
        assert!(run_ops(&ops, &AtomicBool::new(false), |_| Ok(())).is_ok());
    }

    #[test]
    fn the_runner_stops_at_cancellation() {
        let ops = all();
        let cancel = AtomicBool::new(false);
        let mut ran = 0;
        let result = run_ops(&ops, &cancel, |_| {
            ran += 1;
            if ran == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        });
        assert_eq!(result.unwrap_err(), "cancelled");
        assert_eq!(ran, 3);
    }

    #[test]
    fn libtest_summary_must_report_a_passing_test() {
        assert_eq!(
            passed_tests("test result: ok. 16 passed; 0 failed; 0 ignored"),
            Some(16)
        );
        assert_eq!(passed_tests("test result: ok. 0 passed; 0 failed"), Some(0));
        assert_eq!(
            passed_tests("test result: FAILED. 1 passed; 1 failed"),
            None
        );
        assert_eq!(passed_tests(""), None);
    }

    // Fixture roots borrow the checkout's pinned tools by symlink and its configuration by copy.
    mod fixture {
        use crate::tests::scratch;
        use std::fs;

        pub const BAD_RS: &str = "pub fn  f( )->u8{ 1 }\n";
        pub const BAD_MD: &str = "# Title\n\nThis line is deliberately far too long for the configured markdown width because dprint wraps prose at one hundred twenty columns.\n";

        pub fn root() -> tempfile::TempDir {
            let dir = scratch();
            let real = crate::root();
            let p = dir.path();
            for f in ["dev-tools.env", "dprint.json", ".rumdl.toml"] {
                fs::copy(real.join(f), p.join(f)).unwrap();
            }
            fs::create_dir_all(p.join(".tools/bin")).unwrap();
            for t in ["dprint", "rumdl", "actionlint"] {
                std::os::unix::fs::symlink(
                    real.join(".tools/bin").join(t),
                    p.join(".tools/bin").join(t),
                )
                .unwrap();
            }
            fs::create_dir_all(p.join("src")).unwrap();
            fs::write(
                p.join("Cargo.toml"),
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::write(p.join("src/lib.rs"), BAD_RS).unwrap();
            fs::write(p.join("README.md"), BAD_MD).unwrap();
            dir
        }
    }

    #[test]
    fn format_check_never_rewrites_and_write_fixes() {
        let dir = fixture::root();
        let (root, cancel) = (dir.path(), AtomicBool::new(false));
        let e = rustfmt(root, false, &cancel).unwrap_err();
        assert!(e.contains("src/lib.rs"), "{e}");
        let e = dprint(root, "check", &cancel).unwrap_err();
        assert!(e.contains("README.md"), "{e}");
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            fixture::BAD_RS
        );
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            fixture::BAD_MD
        );
        rustfmt(root, true, &cancel).unwrap();
        dprint(root, "fmt", &cancel).unwrap();
        assert_ne!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            fixture::BAD_RS
        );
        assert_ne!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            fixture::BAD_MD
        );
        rustfmt(root, false, &cancel).unwrap();
        dprint(root, "check", &cancel).unwrap();
    }

    #[test]
    fn dprint_with_an_empty_plugin_cache_fails_with_guidance_and_downloads_nothing() {
        let dir = fixture::root();
        let cache = crate::tests::scratch();
        let cancel = AtomicBool::new(false);
        let tool = tools::preflight(dir.path(), Tool::Dprint, &cancel).unwrap();
        let mut cmd = tools::dprint(dir.path(), &tool, "check");
        cmd.env("DPRINT_CACHE_DIR", cache.path());
        let e = tools::explain_dprint(checked(&mut cmd, &cancel).map(|_| ())).unwrap_err();
        assert!(e.contains("./scripts/bootstrap-dev.sh"), "{e}");
        fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        let mut found = Vec::new();
        files(cache.path(), &mut found);
        assert!(
            found
                .iter()
                .all(|f| f.extension().is_none_or(|e| e != "wasm")),
            "{found:?}"
        );
    }

    #[test]
    fn actionlint_ignores_shellcheck_and_pyflakes_on_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture::root();
        let decoys = crate::tests::scratch();
        let marker = decoys.path().join("marker");
        for name in ["shellcheck", "pyflakes"] {
            let path = decoys.path().join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho {name} >> '{}'\n", marker.display()),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let workflows = dir.path().join(".github/workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        // actionlint finds its project by the repository marker.
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let workflow = "on: push\njobs:\n  j:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: echo $x\n      - run: python -c 'import os'\n";
        std::fs::write(workflows.join("ci.yml"), workflow).unwrap();
        let cancel = AtomicBool::new(false);
        let tool = tools::preflight(dir.path(), Tool::Actionlint, &cancel).unwrap();
        let path = format!(
            "{}:{}",
            decoys.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = actionlint_command(dir.path(), &tool);
        cmd.env("PATH", &path);
        checked(&mut cmd, &cancel).expect("clean workflow passes");
        std::fs::write(
            workflows.join("ci.yml"),
            workflow.replace("runs-on", "runs_on"),
        )
        .unwrap();
        let mut cmd = actionlint_command(dir.path(), &tool);
        cmd.env("PATH", &path);
        let e = checked(&mut cmd, &cancel).unwrap_err();
        assert!(e.contains("ci.yml"), "{e}");
        assert!(!marker.exists(), "unrelated tools were invoked");
    }

    #[test]
    fn bash_syntax_error_in_the_second_script_is_reported() {
        let dir = fixture::root();
        let scripts = dir.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("a.sh"), "echo ok\n").unwrap();
        std::fs::write(scripts.join("b"), "if then\n").unwrap();
        let cancel = AtomicBool::new(false);
        bash_syntax(dir.path(), "scripts/a.sh", &cancel).unwrap();
        let e = bash_syntax(dir.path(), "scripts/b", &cancel).unwrap_err();
        assert!(e.contains("scripts/b"), "{e}");
    }

    #[test]
    fn parser_accepts_the_new_verbs_and_rejects_extra_arguments() {
        use crate::{Task, parse};
        let a = |v: &[&str]| v.iter().map(Into::into).collect::<Vec<_>>();
        assert_eq!(parse(&a(&["check"])), Ok(Task::Check));
        assert_eq!(parse(&a(&["lint"])), Ok(Task::Lint));
        assert_eq!(parse(&a(&["architecture"])), Ok(Task::Architecture));
        assert_eq!(parse(&a(&["format"])), Ok(Task::Format { write: false }));
        assert_eq!(
            parse(&a(&["format", "--write"])),
            Ok(Task::Format { write: true })
        );
        for bad in [
            &["check", "x"][..],
            &["lint", "--write"],
            &["architecture", "x"],
            &["format", "--write", "--write"],
            &["format", "x"],
        ] {
            assert!(parse(&a(bad)).is_err(), "{bad:?}");
        }
    }
}
