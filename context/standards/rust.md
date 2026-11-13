# Rust standards

A finished change meets each rule.

- **Naming:** crates are named `winter-*`, modules are snake_case, and there is no `mod.rs`.
- **Visibility:** modules are private by default. The public surface is only the `lib.rs` re-exports. Harness internals
  stay behind the private driver.
- **Dependencies:** versions are declared in the consuming manifest and resolved in the committed `Cargo.lock`. A new
  workspace edge is architectural; see [changes.md](../architecture/changes.md).
- **Errors:** `thiserror` enums or structs that carry context. No stringly errors across the facade.
- **No panics on user input:** no `panic!`, `unwrap` or `expect` on input a user can supply. Panics are allowed for
  internal invariants and in tests.
- **`unsafe`:** only with a comment that justifies it, as in `main.rs`.
- **Warnings** are errors. `winter:lint` and `winter:format-check` enforce Clippy and rustfmt.
- **Feature gating:** gate code with `#[cfg(feature = "dev-sample")]`, and keep production free of the sample.
