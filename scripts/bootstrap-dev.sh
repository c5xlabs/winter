#!/usr/bin/env bash
# Prepares a checkout for development: pinned Rust toolchain, Cargo cache, and repo-local tools.
# Requires network access. Writes only inside the repository plus the rustup, Cargo, and dprint caches.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

die() {
	echo "bootstrap-dev: $*" >&2
	exit 1
}

# 2. Prerequisites (see README "Development").
for tool in rustup cc curl tar; do
	command -v "$tool" >/dev/null 2>&1 || die "missing prerequisite '$tool'; see README.md"
done
if command -v sha256sum >/dev/null 2>&1; then
	sha_check=(sha256sum -c)
elif command -v shasum >/dev/null 2>&1; then
	sha_check=(shasum -a 256 -c)
else
	die "missing prerequisite 'sha256sum' or 'shasum'; see README.md"
fi

# 3. Pins: parsed, never sourced.
DPRINT_VERSION="" RUMDL_VERSION="" ACTIONLINT_VERSION=""
while IFS= read -r line || [[ -n "$line" ]]; do
	[[ "$line" =~ ^[A-Z_]+=[^[:space:]]+$ ]] || die "dev-tools.env: invalid line: $line"
	key="${line%%=*}"
	value="${line#*=}"
	case "$key" in
	DPRINT_VERSION | RUMDL_VERSION | ACTIONLINT_VERSION) ;;
	*) die "dev-tools.env: unknown key: $line" ;;
	esac
	[[ -z "${!key}" ]] || die "dev-tools.env: duplicate key: $line"
	printf -v "$key" '%s' "$value"
done <dev-tools.env
for key in DPRINT_VERSION RUMDL_VERSION ACTIONLINT_VERSION; do
	[[ -n "${!key}" ]] || die "dev-tools.env: missing key: $key"
done

# 4. Rust toolchain from rust-toolchain.toml.
rustup toolchain install

# 5. Cargo cache.
cargo fetch --locked

tools="$repo/.tools"
mkdir -p "$tools/bin"

# 6. dprint and rumdl.
install_cargo_tool() {
	local name="$1" crate="$2" pin="$3"
	if [[ "$("$tools/bin/$name" --version 2>/dev/null || true)" == *" $pin" ]]; then
		echo "reusing $name $pin"
	else
		cargo install --locked --version "$pin" --root "$tools" "$crate"
	fi
}
install_cargo_tool dprint dprint "$DPRINT_VERSION"
install_cargo_tool rumdl rumdl "$RUMDL_VERSION"

# 7. actionlint, verified against the release checksums.
if [[ "$("$tools/bin/actionlint" -version 2>/dev/null | head -n1 || true)" == "$ACTIONLINT_VERSION" ]]; then
	echo "reusing actionlint $ACTIONLINT_VERSION"
else
	case "$(uname -s)" in
	Linux) os=linux ;;
	Darwin) os=darwin ;;
	*) die "unsupported OS for actionlint: $(uname -s)" ;;
	esac
	case "$(uname -m)" in
	x86_64 | amd64) arch=amd64 ;;
	arm64 | aarch64) arch=arm64 ;;
	*) die "unsupported architecture for actionlint: $(uname -m)" ;;
	esac
	archive="actionlint_${ACTIONLINT_VERSION}_${os}_${arch}.tar.gz"
	checksums="actionlint_${ACTIONLINT_VERSION}_checksums.txt"
	base="https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}"
	work="$(mktemp -d)"
	trap 'rm -rf "$work"' EXIT
	(
		cd "$work"
		curl -fsSL -o "$archive" "$base/$archive"
		curl -fsSL -o "$checksums" "$base/$checksums"
		grep -F "  $archive" "$checksums" >expected || die "no checksum line for $archive"
		"${sha_check[@]}" expected || die "checksum mismatch for $archive"
		tar -xzf "$archive" actionlint
	)
	install -m 0755 "$work/actionlint" "$tools/bin/actionlint"
	[[ "$("$tools/bin/actionlint" -version | head -n1)" == "$ACTIONLINT_VERSION" ]] ||
		die "installed actionlint does not report $ACTIONLINT_VERSION"
	echo "installed actionlint $ACTIONLINT_VERSION (checksum verified)"
fi

# 8. Warm the dprint plugin cache.
"$tools/bin/dprint" output-resolved-config >/dev/null
echo "bootstrap complete"
