# Changing the architecture

## Adding a command

Touch these in order. Each step names the method that verifies it (see [the matrix](../verification/matrix.md)).

1. `crates/platform/src/cli.rs`: the `Command` built in `command()`, a `Request` or `Control` variant, `parse`, and the
   reply renderer. Verify with `winter:unit-test`.
2. `crates/platform/src/lib.rs`: re-export, only for new public types. Verify with `winter:architecture`.
3. `crates/cli/src/main.rs`: the match arm. Verify with `winter:unit-test`.
4. Tests: inline unit tests in `cli.rs`; a facade case in `crates/test-harness/tests/component.rs`
   (`winter:component-test`); an executable case in `crates/test-harness/tests/service.rs` (`winter:service-test`).

Gate a development-only command behind `#[cfg(feature = "dev-sample")]`, as `sample` is, and verify it with
`winter:sample-test` too. Finish with `winter:check`.

## Routine or architectural

A command-local change inside the existing modules and facade is routine and needs no project-level decision.

These are architectural decisions:

- a new package
- a workspace dependency edge
- a facade contract other packages rely on
- stream or exit-code ownership
- executable identity
- the configuration and feature chain

## Boundary change procedure

1. Run `winter:architecture` (`cargo dev architecture`) before the change.
2. Edit the allowlist in `crates/test-harness/src/architecture.rs` and the package table in
   [structure.md](./structure.md) in the same change.
3. Run `winter:architecture` again.

Reviewers apply `wctx:concept-dependency-graph`, `wctx:internal-visibility` and `wctx:shared-kernel`.

## Staying inside the boundaries

`winter:architecture` stays green, and the facade exports stay explicit.
