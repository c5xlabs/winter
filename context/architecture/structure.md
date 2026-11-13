# Structure

## Packages

The workspace has exactly four members. `crates/test-harness/src/architecture.rs` enforces the allowed edges, and `cargo
dev architecture` runs it.

| Package               | Target                       | Responsibility                                        | May depend on     |
| --------------------- | ---------------------------- | ----------------------------------------------------- | ----------------- |
| `winter-platform`     | library                      | Command parsing and reply rendering                   | nothing           |
| `winter-cli`          | binary `winter`              | Process entry point: streams and exit codes           | `winter-platform` |
| `winter-test-harness` | library, `winter-dev` driver | Shared fixtures, the architecture checker, the driver | `winter-platform` |
| `winter-dev-sample`   | binary                       | Development-only sample executable                    | nothing           |

Every declared edge counts, whatever its kind, `optional` flag, rename or target condition.

## `winter-platform` modules

- `lib.rs` is the facade. It re-exports each public item explicitly; it has no glob exports.
- `cli.rs`, `sample.rs` and `process.rs` are private modules. `sample.rs` and `process.rs` exist only under the
  `dev-sample` feature.

## Wiring and configurations

- `winter-platform` is declared once, in `[workspace.dependencies]`.
- The `dev-sample` feature chains from `winter-cli` and `winter-test-harness` to `winter-platform`.
- Production builds omit the feature and contain no sample. Development builds enable it. The driver builds both under
  `target/verification/<configuration>`.

## Conventions

- `cli.rs::command` builds the clap `Command`. `parse` returns a `Request`, discards argv[0], and never passes the raw
  `sample` tail to clap.
- The platform returns `Reply`, `UsageError` or `LaunchError` values and never writes a stream.
- `crates/cli/src/main.rs` alone writes stdout and stderr and maps exit codes: 0 for success, 1 on a stream failure, 2
  on a usage error.
- `CliIdentity.version` comes from `winter-cli`'s `CARGO_PKG_VERSION`. It never comes from platform metadata, argv[0] or
  the working directory.

## Shared rules realized here

`wctx:concept-dependency-graph`, `wctx:internal-visibility`, and the clean-architecture composition root (the CLI crate
wires the platform to the process).

To change any of this, follow [changes.md](./changes.md).
