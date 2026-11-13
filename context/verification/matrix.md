# Verifiability matrix

Run every command from the checkout root after `./scripts/bootstrap-dev.sh`. The shape follows
`canon:verifiability-matrix`.

## Commands

| ID                      | Command                                               | Scope                                                                  |
| ----------------------- | ----------------------------------------------------- | ---------------------------------------------------------------------- |
| `winter:check`          | `cargo dev check`                                     | Every gate below plus build and tests.                                 |
| `winter:format-check`   | `cargo dev format`                                    | rustfmt, dprint (Markdown, JSON, TOML, workflow YAML, both scripts).   |
| `winter:lint`           | `cargo dev lint`                                      | Clippy (both configurations), rumdl, actionlint, per-script `bash -n`. |
| `winter:architecture`   | `cargo dev architecture`                              | Dependency allowlist and its self-tests.                               |
| `winter:build`          | `cargo dev build --configuration production`          | Build the production CLI.                                              |
| `winter:unit-test`      | `cargo dev test unit --configuration production`      | Unit and library doctests.                                             |
| `winter:component-test` | `cargo dev test component --configuration production` | In-process facade.                                                     |
| `winter:service-test`   | `cargo dev test service --configuration production`   | Built executable and isolated runtime.                                 |
| `winter:sample-test`    | `cargo dev test service --configuration development`  | Built development CLI and sample.                                      |

Code gated by `dev-sample` is also run in the matching tier with `--configuration development`. `winter:check` runs both
configurations.

## Manual testing

Steps are in [exercises.md](./exercises.md).

### `winter:manual`

Production artifact selection, isolation and wrapper behavior. Setup: bootstrapped checkout with Python 3. Success: help
and version print, production `sample` exits 2, no decoy marker appears. See
[the production exercise](./exercises.md#production-terminal-exercise).

### `winter:manual-sample`

The development sample handoff. Setup: bootstrapped checkout, interactive shell. Success: the JSON line, exit codes and
process ancestry match. See [the sample exercise](./exercises.md#development-sample-exercise).

### `winter:manual-gates`

The gates on the final tree. Setup: bootstrapped checkout. Success: `cargo dev check` exits 0, and each injected failure
names the right operation. See [the gate exercise](./exercises.md#gate-exercise).

### `winter:manual-ci`

CI after the branch is pushed. Setup: pushed branch and `gh`. Success: all four matrix jobs are green. See
[the CI exercise](./exercises.md#ci-exercise).

### `winter:manual-context-eval`

Cold-agent discovery and behavior of this context. Setup: fresh foreground agents. Success: each scenario is reported
reached and behaved. See [the evaluation](./exercises.md#cold-agent-context-evaluation).

### `winter:manual-context-references`

Every route and citation in this context. Setup: the operator's workspace. Success: every leaf is reached and every
`wctx:` or `canon:` citation resolves. See
[the references exercise](./exercises.md#context-references-exercise-wintermanual-context-references).

## Tools

| ID            | Command                       | Purpose                                               |
| ------------- | ----------------------------- | ----------------------------------------------------- |
| `tool:cli`    | `./scripts/winter-cli --help` | Run this checkout's CLI from any directory.           |
| `tool:sample` | `cargo dev sample -- inspect` | Development-only demonstration of the sample handoff. |
