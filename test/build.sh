#!/bin/sh
# Build what stormd's test image packages, for the commit checked out:
#
#   test/build.sh [target]      default x86_64-unknown-linux-musl
#
# Per stormcentral docs/test-standard.md the runner runs this first, in the
# checkout with CARGO_TARGET_DIR set, then reads test/Containerfile itself
# (no container runtime, stormcentral#121) with the repo root as context. So
# this builds and stops: the two static binaries land in test/out/ (ignored by
# git), where the Containerfile COPYs them from.
#
# stormd depends on stormpull over ssh:// (a private repo), so the binaries
# are built here, where cargo already has access, not in an image build.
# Needs the musl target and its linker (.cargo/config.toml), as for any
# stormd release build.
set -eu
target=${1:-x86_64-unknown-linux-musl}
root=$(cd "$(dirname "$0")/.." && pwd)

cargo build --release --locked --target "$target" -p stormd -p stormd-test --manifest-path "$root/Cargo.toml"
tdir=$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" |
    sed 's/.*"target_directory":"\([^"]*\)".*/\1/')
out="$root/test/out"
rm -rf "$out"
mkdir -p "$out"
cp "$tdir/$target/release/stormd" "$tdir/$target/release/stormd-test" "$out/"
echo "$out"
