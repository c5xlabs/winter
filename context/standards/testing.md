# Testing

This file owns where each test lives. For the tier rules, see `wctx:test-tiers` and `wctx:tests-follow-the-subject`. For
process fixtures, see `winter-context:/standards/testing/process-fixtures.md`.

| Tier      | Location                                        | Subject                                                                                    |
| --------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Unit      | Inline `#[cfg(test)] mod tests` beside the code | Package internals, binary units and doctests                                               |
| Component | `crates/test-harness/tests/component.rs`        | The platform facade in process, with a controlled `Process` double; never launches the CLI |
| Service   | `crates/test-harness/tests/service.rs`          | Built executables through `RuntimeFixture` and `invoke`; run only through the driver       |

- Shared fixtures live in the `winter-test-harness` library and are never copied between suites.
- Production code never depends on the harness.
- Focus a run with `--filter TEXT`, for example `cargo dev test service --configuration development --filter
  executable_help_and_version`. A filter can match no tests, so report what matched.
