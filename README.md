# winter

Winter platform CLI launcher

## Development

Host prerequisites: rustup 1.28 or newer, a C linker toolchain (`cc`), Bash, curl, tar, and `sha256sum` or `shasum`.

Run `./scripts/bootstrap-dev.sh` once per checkout. It needs network access and is never run implicitly. It installs the
pinned Rust toolchain from `rust-toolchain.toml`, fetches the locked crates, and installs the formatter and linters
pinned in `dev-tools.env` into the ignored `.tools/` directory. Re-running reuses tools that already match their pins.

After bootstrap, everything works offline:

```sh
cargo build --locked --offline
cargo run --locked --offline -- --help
cargo test --locked --offline --workspace
```

The development configuration, which enables the `dev-sample` feature, uses a separate target directory so it never
mixes with production artifacts:

```sh
cargo build --locked --offline --target-dir target/development --features winter-cli/dev-sample
cargo test --locked --offline --target-dir target/development --workspace \
	--features winter-platform/dev-sample,winter-cli/dev-sample,winter-test-harness/dev-sample
```

Formatting and linting: `cargo fmt --all -- --check`, `.tools/bin/dprint check`, and `.tools/bin/rumdl check .`.

`winter` is never installed from this repository and `cargo install` is not part of the workflow. Run the checkout
binary by its `target/` path, for example `target/debug/winter`.
