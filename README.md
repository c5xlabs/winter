# winter

Winter platform CLI launcher

To run this checkout's CLI (for example to check its version) without touching an installed `winter`, use the absolute
path to `scripts/winter-cli`; see Development below.

## Development

Host prerequisites: rustup 1.28 or newer, a C linker toolchain (`cc`), Bash, curl, tar, and `sha256sum` or `shasum`.

Run `./scripts/bootstrap-dev.sh` explicitly once per checkout, with network access. It installs the pinned Rust
toolchain, fetches locked crates, installs tools pinned in `dev-tools.env` into `.tools/`, and warms dprint's plugin
cache. Re-running reuses matching tools. Bootstrap also uses the rustup, Cargo and dprint caches. The commands below use
locked, offline Cargo operations; offline use requires those caches and tools to have been populated for the host and
any chosen target. Bootstrap is never implicit.

The checkout's `cargo dev` alias runs the private `winter-dev` driver. From the repository root:

```sh
cargo dev build --configuration production
cargo dev build --configuration development
cargo dev test unit --configuration production
cargo dev test component --configuration development
cargo dev test service --configuration production
cargo dev test service --configuration development --filter executable_help_and_version
cargo dev check
cargo dev format
cargo dev format --write
cargo dev lint
cargo dev architecture
cargo dev run -- --help
cargo dev sample -- inspect
./scripts/winter-cli --version
```

Build and test require an explicit `production` or `development` configuration. Test tiers are `unit`, `component` and
`service`; `--filter TEXT` passes a Cargo test-name filter to the selected tier (a filter can match no tests). Unit
tests cover package internals and library doctests; component tests exercise the platform facade in process; service
tests exercise built executables in isolated subprocess fixtures. Run both configurations and all relevant tiers when
checking a change. Development enables the test-only sample; production excludes it. `run --` builds development
artifacts and executes the checkout CLI, forwarding arguments, working directory, streams and process context. The
wrapper does the same from any working directory, selecting this checkout's pinned toolchain and manifest.

`cargo dev check` is the full local gate and is exactly what CI runs. It runs one ordered list of named operations:
`rustfmt-check`, `dprint-check`, `clippy-production`, `clippy-development`, `rumdl`, `actionlint`,
`bash-syntax-bootstrap`, `bash-syntax-winter-cli`, `architecture-self-test`, `architecture-allowlist`,
`build-production`, `build-development`, then the unit, component and service tiers in both configurations. It prints
`winter-dev: [i/N] <name>` before each operation and `ok` or `FAILED` after it with the failed operation's retained
diagnostics, keeps going after a failure, and ends with `winter-dev: failed checks: <names>` and exit 1. `format`,
`lint` and `architecture` run their slice of the same list. They and `check` never rewrite files; only `cargo dev format
--write` does (`cargo fmt --all` and `dprint fmt`). Extra arguments are a usage error (exit 2); failed checks, missing
prerequisites, timeouts and orchestration failures exit 1.

Before using a pinned tool, the driver verifies it with a bounded version probe of `.tools/bin/<tool>` against
`dev-tools.env`, and checks that `cargo fmt` and `cargo clippy` exist in the pinned toolchain. A missing or mismatched
tool fails with the expected pin, what was found, and `run ./scripts/bootstrap-dev.sh (network required)`; the driver
never installs or downloads anything (`RUSTUP_AUTO_INSTALL=0`, and dprint runs with a closed proxy so a missing plugin
cache entry fails with the same guidance). actionlint runs with its shellcheck and pyflakes integrations disabled, so
unrelated tools on `PATH` cannot change the result. After bootstrap `cargo dev check` runs offline and without services.
Focused `cargo dev test <tier> --configuration <c> --filter TEXT` remains the quick feedback loop: it runs one tier and
reuses the same build artifacts as the full gate.

The architecture check (`cargo dev architecture`) first runs the checker's self-tests, then applies the allowlist to the
workspace's `cargo metadata --no-deps` output: platform depends on nothing, `winter-cli` and `winter-test-harness` on
`winter-platform` only, `winter-dev-sample` on nothing. Every declared workspace edge counts, whatever its kind (normal,
dev or build), `optional`, rename or target condition, and the workspace must have exactly these four members. Each
diagnostic names both packages and the kind, optional, target and alias of the edge. To change a boundary, edit the
allowlist in `crates/test-harness/src/architecture.rs` together with the package table in the system specification, as
an architectural decision.

CI (`.github/workflows/check.yml`) runs on push and pull request with read-only repository permissions on
`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-15` and `macos-15-intel`. Each job bootstraps (reusing a cache of `.tools`
and the dprint plugin cache that bootstrap still verifies) and runs `cargo dev check` unchanged. It has no secrets,
release triggers, publication, signing, Homebrew or installed Winter; release readiness is a separate concern.

`winter sample [ARG...]` exists only in the development configuration. It hands everything after `sample` to the
development sample executable byte-for-byte, replacing the `winter` process (same PID, no supervising parent). The
driver injects `WINTER_DEV_SAMPLE_PATH` with the absolute sample path; do not set it by hand or use an installed
`winter`. `cargo dev sample -- ARG...` is shorthand for `cargo dev run -- sample ARG...`. Production rejects `sample` as
an unknown command and never reads that variable.

The driver uses `target/verification/production` and `target/verification/development`, separate from one another and
from ordinary Cargo targets, with warnings denied. It selects binaries from Cargo metadata and compiler-artifact JSON,
not a guessed `target/debug` path; this matters with configured targets and architectures. Cargo's build environment,
including target configuration and flags, is distinct from the cleared runtime environment of service fixtures. The
service driver supplies absolute artifact paths, expected package version and an owned process scope to its tests;
calling the service test target directly without these inputs is unsupported. See the
[test harness documentation](crates/test-harness/src/lib.rs) for fixture ownership and the input contract.

This checkout does not install `winter` or change an existing installation. Use `cargo dev run -- ...` or the absolute
path to `scripts/winter-cli` rather than an unqualified `winter`; an installed workspace CLI can coexist on PATH.

### Verification inventory

#### Commands

| ID                      | Command                                                                                                          | Scope                                                                  |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| `winter:build`          | `cargo dev build --configuration production` and `cargo dev build --configuration development`                   | Build each configuration.                                              |
| `winter:unit-test`      | `cargo dev test unit --configuration production` and `cargo dev test unit --configuration development`           | Unit and library doctests.                                             |
| `winter:component-test` | `cargo dev test component --configuration production` and `cargo dev test component --configuration development` | In-process facade.                                                     |
| `winter:service-test`   | `cargo dev test service --configuration production` and `cargo dev test service --configuration development`     | Built executable and isolated runtime.                                 |
| `winter:check`          | `cargo dev check`                                                                                                | Every gate below plus build and tests.                                 |
| `winter:format-check`   | `cargo dev format`                                                                                               | rustfmt, dprint (Markdown, JSON, TOML, workflow YAML, both scripts).   |
| `winter:lint`           | `cargo dev lint`                                                                                                 | Clippy (both configurations), rumdl, actionlint, per-script `bash -n`. |
| `winter:architecture`   | `cargo dev architecture`                                                                                         | Dependency allowlist and its self-tests.                               |

#### Manual testing

| ID                           | Exercise                                                                                               |
| ---------------------------- | ------------------------------------------------------------------------------------------------------ |
| `winter:manual`              | Perform the terminal exercise below for production artifact selection, isolation and wrapper behavior. |
| `winter:manual-sample`       | Perform the development sample exercise below.                                                         |
| `winter:manual-gates`        | Perform the gate exercise below on the final tree.                                                     |
| `winter:manual-ci`           | Perform the CI exercise below after pushing the branch.                                                |
| `winter:manual-context-eval` | Run the four cold-agent discovery scenarios below.                                                     |

The following exercise needs Python 3 in addition to the host prerequisites. Run from the checkout root after bootstrap.
It selects the production executable by package ID and bin target from Cargo's JSON, so a configured cross-target path
is not guessed. A cross-compiled executable must be runnable on the host for the runtime checks.

```sh
set -e
repo="$PWD"
work="$(mktemp -d "${BLIZZARD_TMPDIR:-${TMPDIR:-/tmp}}/winter-manual.XXXXXX")"
mkdir -p "$work/bin" "$work/cwd" "$work/home" "$work/config" "$work/cache" "$work/data" "$work/state" "$work/tmp"
printf '#!/bin/sh\nprintf decoy > "$MARKER"\n' >"$work/bin/winter"
chmod +x "$work/bin/winter"
printf '[toolchain]\nchannel = "nonexistent-conflicting-toolchain"\n' >"$work/cwd/rust-toolchain.toml"
cargo metadata --locked --offline --no-deps --format-version 1 >"$work/metadata.json"
CARGO_TARGET_DIR="$repo/target/verification/production" cargo build --locked --offline --no-default-features \
	-p winter-cli --bin winter --message-format=json >"$work/artifacts.jsonl"
cli="$(
	python3 - "$work/metadata.json" "$work/artifacts.jsonl" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    packages = [p for p in json.load(f)['packages'] if p['name'] == 'winter-cli']
assert len(packages) == 1
with open(sys.argv[2]) as f:
    messages = [json.loads(line) for line in f]
paths = [m['executable'] for m in messages if m.get('reason') == 'compiler-artifact'
         and m.get('package_id') == packages[0]['id'] and m['target']['name'] == 'winter'
         and 'bin' in m['target']['kind'] and m.get('executable')]
assert len(paths) == 1
print(paths[0])
PY
)"
(
	cd "$work/cwd"
	env -i HOME="$work/home" XDG_CONFIG_HOME="$work/config" XDG_CACHE_HOME="$work/cache" \
		XDG_DATA_HOME="$work/data" XDG_STATE_HOME="$work/state" TMPDIR="$work/tmp" \
		PATH="$work/bin" MARKER="$work/decoy-ran" "$cli" --help
	env -i HOME="$work/home" PATH="$work/bin" MARKER="$work/decoy-ran" "$cli" --version
	env -i MARKER="$work/decoy-ran" "$work/bin/winter"
	test -e "$work/decoy-ran"
	rm "$work/decoy-ran"
	if env -i HOME="$work/home" PATH="$work/bin" MARKER="$work/decoy-ran" "$cli" sample; then
		echo 'production unexpectedly accepted sample' >&2
		exit 1
	else
		test "$?" -eq 2
	fi
	test ! -e "$work/decoy-ran"
	PATH="$work/bin:$PATH" MARKER="$work/decoy-ran" "$repo/scripts/winter-cli" --version
	test ! -e "$work/decoy-ran"
)
printf 'Inspect the output and temporary tree: %s\n' "$work"
```

Confirm help and version are printed, the production `sample` request is rejected with exit 2, and no decoy marker or
unexpected runtime files appear. The wrapper must print this checkout's version despite the conflicting toolchain and
PATH decoy. Remove the printed temporary tree when finished.

#### Development sample exercise

Run from the checkout root after bootstrap, in an interactive shell.

1. `cargo dev sample -- inspect --help $'\xff'` prints one JSON line. Its `args_hex` is
   `["696e7370656374","2d2d68656c70","ff"]`, `env_hex.WINTER_DEV_SAMPLE_PATH` is `null`, and `cwd_hex` is the current
   directory.
2. `cargo dev sample -- --help` prints the sample usage. `cargo dev sample -- -- --help; echo $?` prints `2`, and `cargo
   dev sample -- exit 7; echo $?` prints `7`.
3. Run `cargo dev sample -- wait &`. After `ready` appears, `ps -o pid,ppid,comm -C winter-dev-sample` shows the sample
   in place of `winter`; its parent is whatever launched `cargo dev` (Cargo replaces itself), and no `winter` or
   `winter-dev` process is in its ancestry. Send it `kill -TERM` and confirm it ends by SIGTERM.
4. Build the development CLI and select it by package ID from Cargo JSON, as in the production exercise but with
   `--features dev-sample` and the `development` target directory. Then `env -u WINTER_DEV_SAMPLE_PATH "$cli" sample`
   and `WINTER_DEV_SAMPLE_PATH=rel "$cli" sample` each exit 1 with a `development sample handoff failed` diagnostic and
   empty stdout.
5. Repeat steps 1 and 2 with the network disabled (for example `unshare -rn`) to confirm they need no network after
   bootstrap. If the host has no usable network-isolation tool, record this step as not run and say why.

#### Gate exercise

Run from the checkout root after bootstrap. Restore each injected change with `git checkout -- <file>` and confirm `git
status` is clean at the end.

1. `cargo dev check` exits 0, its log shows every named operation in order, and `git status --porcelain` is unchanged.
2. Inject one failure at a time. Each run of `cargo dev check` (or the named sub-command) exits 1 and names the right
   operation:
   - a rustfmt violation → `rustfmt-check`
   - a Markdown wrap violation → `dprint-check`
   - an unused variable → `clippy-*` and `build-*`
   - a `clippy::needless_return` → `clippy-*`
   - a rumdl violation → `rumdl`
   - a workflow typo → `actionlint`
   - a syntax error in `scripts/winter-cli` → `bash-syntax-winter-cli`
   - a failing `assert!` in a unit test → `test-unit-*`
   - each of these `crates/cli/Cargo.toml` edges → `architecture-allowlist`, with package names and kind or condition in
     the diagnostic: `[target.'cfg(windows)'.dependencies]` with `harness = { package = "winter-test-harness", path =
     "../test-harness", optional = true }`, a `[build-dependencies]` edge to `winter-dev-sample`, and a
     `[dev-dependencies]` edge to `winter-test-harness`. After each edit run `cargo metadata --offline >/dev/null` so
     the locked driver still builds, and restore `Cargo.lock` too.
3. `cargo dev format` on a misformatted tree leaves files byte-identical (compare `git diff --stat` before and after);
   `cargo dev format --write` fixes them.
4. Pinned-tool failures: renaming `.tools/bin/rumdl` makes `cargo dev lint` exit 1 with bootstrap guidance and download
   nothing; `RUMDL_VERSION=0.0.0` in a scratch edit of `dev-tools.env` gives a mismatch diagnostic; pointing
   `DPRINT_CACHE_DIR` at an empty directory makes `cargo dev format` fail with plugin-cache guidance and leaves that
   directory without plugins.
5. With executable marker-writing `shellcheck` and `pyflakes` files first on `PATH`, `cargo dev lint` writes no marker.
6. Run `cargo dev check` with the network disabled (`unshare -rn` where permitted; otherwise proxy variables pointed at
   a closed loopback port, and record which): it passes. Then `cargo dev test unit --configuration production --filter
   architecture` runs only that tier with no `Compiling` lines for unchanged crates and no format or lint sweep.

#### CI exercise

1. Push the branch. With `gh run list --branch <branch>` and `gh run view <id>`, all four matrix jobs succeed, and each
   log shows bootstrap followed by the same `winter-dev: [i/N] ...` operation sequence.
2. Read `.github/workflows/check.yml`: no secrets, release or tag triggers, or publish steps, and the job token is
   `contents: read`.
3. Push one commit with a single injected rustfmt violation to a throwaway branch, never the feature branch. Each job
   fails, the log names `rustfmt-check` with its diff and the final `failed checks` line. Delete the throwaway branch
   with `git push origin --delete <branch>`.

#### Cold-agent context evaluation

Run each scenario in a separate, fresh foreground agent session with the normal workspace instructions and discovery
chain, but no conversation history, prior findings, or hints about this README. Give the agent only the quoted task and
the checkout location. Observe its file reads and actions; do not direct it to the expected documentation or commands.
The intended route is the workspace instructions and project context → this repository → `README.md` → Development and
Verification inventory. Record that route and the actual files consulted for each run.

- **Filtered service test cue:** “Check the executable help and version service test in the development configuration
  for this checkout, and tell me what ran.”
  - **Reached:** The agent discovers and reads the development test guidance in this README through the normal route.
  - **Behaved:** It uses the checkout driver for the development service tier with an appropriate test-name filter, not
    the service test target directly. The driver supplies the owned process scope and artifact inputs even for a
    filtered run; the agent reports what matched and does not treat zero matches as a passing test.
- **Development sample cue:** “Show me the development sample receiving a non-UTF-8 argument from this checkout's CLI.”
  - **Reached:** The agent discovers and reads the development sample guidance in this README through the normal route.
  - **Behaved:** It uses `cargo dev sample -- ...` (or `cargo dev run -- sample ...`) rather than an installed `winter`
    or a hand-set `WINTER_DEV_SAMPLE_PATH`.
- **Local verification cue:** “Run this checkout's full local verification before I push and tell me what failed.”
  - **Reached:** The agent discovers and reads the Development and Verification inventory guidance in this README
    through the normal route.
  - **Behaved:** It runs `cargo dev check` rather than ad hoc cargo, dprint or rumdl invocations, and reports the named
    failed checks from its summary line.
- **Checkout CLI cue:** “From an unrelated working directory, check this checkout's CLI version without changing my
  installed CLI.”
  - **Reached:** The agent discovers and reads the checkout CLI guidance in this README through the normal route.
  - **Behaved:** It invokes the checkout's wrapper by absolute path from the unrelated directory, rather than an
    unqualified installed `winter`, and does not install or replace a CLI. The wrapper selects this checkout's toolchain
    and manifest.

For each scenario report **reached?** and **behaved?** separately, with evidence from the observed reads, commands, and
output. If the context was not reached, repair the discovery route; if reached but behavior was wrong, repair the
guidance. If behavior was correct without reaching the context, run a control without the guidance to assess whether it
contributed. Rerun failed scenarios with fresh agents after repair. If fresh foreground agents cannot be started, report
the eval as not run and hand it to a session that can start them; a warm agent or the terminal exercise above is not a
substitute.

#### Tools

| ID            | Tool                                                                                               |
| ------------- | -------------------------------------------------------------------------------------------------- |
| `tool:cli`    | `cargo dev` and `scripts/winter-cli`: checkout-local build, test and development CLI entry points. |
| `tool:sample` | `cargo dev sample -- inspect`: development-only demonstration of the sample handoff.               |
