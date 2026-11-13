# Manual exercises

Each exercise runs from the checkout root after `./scripts/bootstrap-dev.sh`. The method IDs are defined in
[matrix.md](./matrix.md).

## Production terminal exercise

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

## Development sample exercise

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

## Gate exercise

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

## CI exercise

1. Push the branch. With `gh run list --branch <branch>` and `gh run view <id>`, all four matrix jobs succeed, and each
   log shows bootstrap followed by the same `winter-dev: [i/N] ...` operation sequence.
2. Read `.github/workflows/check.yml`: no secrets, release or tag triggers, or publish steps, and the job token is
   `contents: read`.
3. Push one commit with a single injected rustfmt violation to a throwaway branch, never the feature branch. Each job
   fails, the log names `rustfmt-check` with its diff and the final `failed checks` line. Delete the throwaway branch
   with `git push origin --delete <branch>`.

## Context references exercise (`winter:manual-context-references`)

Starting from `AGENTS.md`, follow every route. Success means all of the following:

- Every leaf under `context/` is reached from `AGENTS.md`.
- Every `wctx:` id resolves to exactly one heading under the operator's `winter-context` governance, and every `canon:`
  id resolves under `winter-canon`.
- No shared-governance text is copied, and there is no pin, URL-at-commit or fetch script.
- Nothing routes to `legacy-winter-context`.

## Cold-agent context evaluation

Run each scenario in a separate, fresh foreground agent session with the normal workspace instructions and discovery
chain, but no conversation history, prior findings, or hints about this documentation. Give the agent only the quoted
task and the checkout location. Observe its file reads and actions; do not direct it to the expected documentation or
commands. The intended route is the workspace instructions and project context → this repository's `AGENTS.md` →
`context/index.md` → the leaf for the task. Record that route and the actual files consulted for each run.

- **Filtered service test cue:** “Check the executable help and version service test in the development configuration
  for this checkout, and tell me what ran.”
  - **Reached:** The agent discovers and reads the development test guidance in this repository's context through the
    normal route.
  - **Behaved:** It uses the checkout driver for the development service tier with an appropriate test-name filter, not
    the service test target directly. The driver supplies the owned process scope and artifact inputs even for a
    filtered run; the agent reports what matched and does not treat zero matches as a passing test.
- **Development sample cue:** “Show me the development sample receiving a non-UTF-8 argument from this checkout's CLI.”
  - **Reached:** The agent discovers and reads the development sample guidance in this repository's context through the
    normal route.
  - **Behaved:** It uses `cargo dev sample -- ...` (or `cargo dev run -- sample ...`) rather than an installed `winter`
    or a hand-set `WINTER_DEV_SAMPLE_PATH`.
- **Local verification cue:** “Run this checkout's full local verification before I push and tell me what failed.”
  - **Reached:** The agent discovers and reads the verification and tooling guidance in this repository's context
    through the normal route.
  - **Behaved:** It runs `cargo dev check` rather than ad hoc cargo, dprint or rumdl invocations, and reports the named
    failed checks from its summary line.
- **Checkout CLI cue:** “From an unrelated working directory, check this checkout's CLI version without changing my
  installed CLI.”
  - **Reached:** The agent discovers and reads the checkout CLI guidance in this repository's context through the normal
    route.
  - **Behaved:** It invokes the checkout's wrapper by absolute path from the unrelated directory, rather than an
    unqualified installed `winter`, and does not install or replace a CLI. The wrapper selects this checkout's toolchain
    and manifest.

For each scenario report **reached?** and **behaved?** separately, with evidence from the observed reads, commands, and
output. If the context was not reached, repair the discovery route; if reached but behavior was wrong, repair the
guidance. If behavior was correct without reaching the context, run a control without the guidance to assess whether it
contributed. Rerun failed scenarios with fresh agents after repair. If fresh foreground agents cannot be started, report
the eval as not run and hand it to a session that can start them; a warm agent or the terminal exercise above is not a
substitute.

Three more scenarios cover implementation and review:

- **Implement cue:** “Plan adding a `winter doctor` command to this checkout: which files change and how is it
  verified?”
  - **Reached:** The agent reads `context/architecture/changes.md`.
  - **Behaved:** It names the `cli.rs`, `main.rs` and three test-file touchpoints and the matrix IDs, and treats the
    change as routine, not architectural.
- **Code review cue:** review a fixture diff in which the platform `println!`s, unwraps user input, and reads the
  platform's own `CARGO_PKG_VERSION`.
  - **Reached:** The agent reads `context/standards/rust.md` and `context/architecture/structure.md`.
  - **Behaved:** It flags all three and cites the document.
- **Architecture review cue:** review a fixture diff adding a `winter-platform` → `winter-test-harness` dependency.
  - **Reached:** The agent reads `context/architecture/changes.md`.
  - **Behaved:** It flags an architectural decision, names `cargo dev architecture` and `wctx:concept-dependency-graph`,
    and requires the allowlist and package table to be updated together.
