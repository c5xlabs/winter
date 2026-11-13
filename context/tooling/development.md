# Development

Tier meanings are owned by [testing.md](../standards/testing.md); method IDs by [the matrix](../verification/matrix.md).

## Fresh checkout

This walkthrough needs no Python, installation, services or credentials. Python 3 is needed only for the `winter:manual`
exercise.

1. `./scripts/bootstrap-dev.sh` (network required).
2. `cargo dev build --configuration production`
3. `cargo dev test service --configuration production --filter executable_help_and_version`
4. `cd /tmp && "<checkout>/scripts/winter-cli" --version`, from an unrelated directory.
5. `cargo dev sample -- inspect`

## Setup and commands

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
allowlist in `crates/test-harness/src/architecture.rs` together with the package table in
[structure.md](../architecture/structure.md), as an architectural decision (see
[changes.md](../architecture/changes.md)).

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
[test harness documentation](../../crates/test-harness/src/lib.rs) for fixture ownership and the input contract.

## Checkout CLI

`scripts/winter-cli` resolves this checkout's manifest and toolchain from any working directory. It does not rename the
public `winter` executable or its help and version identity, and needs no alias, profile or PATH change.

This checkout does not install `winter` or change an existing installation. Use `cargo dev run -- ...` or the absolute
path to `scripts/winter-cli` rather than an unqualified `winter`; an installed workspace CLI can coexist on PATH.
