# winter

Winter platform CLI launcher

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
cargo dev run -- --help
./scripts/winter-cli --version
```

Build and test require an explicit `production` or `development` configuration. Test tiers are `unit`, `component` and
`service`; `--filter TEXT` passes a Cargo test-name filter to the selected tier (a filter can match no tests). Unit
tests cover package internals and library doctests; component tests exercise the platform facade in process; service
tests exercise built executables in isolated subprocess fixtures. Run both configurations and all relevant tiers when
checking a change. Development enables the test-only sample; production excludes it. `run --` builds development
artifacts and executes the checkout CLI, forwarding arguments, working directory, streams and process context. The
wrapper does the same from any working directory, selecting this checkout's pinned toolchain and manifest.

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

| ID                           | Command                                                                                                          | Scope                                  |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| `winter:build`               | `cargo dev build --configuration production` and `cargo dev build --configuration development`                   | Build each configuration.              |
| `winter:unit-test`           | `cargo dev test unit --configuration production` and `cargo dev test unit --configuration development`           | Unit and library doctests.             |
| `winter:component-test`      | `cargo dev test component --configuration production` and `cargo dev test component --configuration development` | In-process facade.                     |
| `winter:service-test`        | `cargo dev test service --configuration production` and `cargo dev test service --configuration development`     | Built executable and isolated runtime. |
| `winter:source-format-check` | `cargo fmt --all -- --check` and `.tools/bin/dprint check`                                                       | Source formatting.                     |
| `winter:prose-config-check`  | `.tools/bin/rumdl check .`                                                                                       | Markdown and its configuration.        |
| `winter:shell-syntax`        | `bash -n scripts/bootstrap-dev.sh scripts/winter-cli`                                                            | Shell syntax.                          |

#### Manual testing

| ID                           | Exercise                                                                                                                                                      |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `winter:manual`              | Perform the terminal exercise below for production artifact selection, isolation and wrapper behavior.                                                        |
| `winter:manual-context-eval` | From an unrelated directory, invoke the absolute wrapper path and confirm it uses this checkout rather than the caller's toolchain or a PATH decoy, as below. |

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

#### Tools

| ID         | Tool                                                                                               |
| ---------- | -------------------------------------------------------------------------------------------------- |
| `tool:cli` | `cargo dev` and `scripts/winter-cli`: checkout-local build, test and development CLI entry points. |
