#!/bin/sh
# Build stormd's test container, stormd-test-<suite>, for the commit checked out.
#
#   test/build.sh [suite] [target]      e.g. test/build.sh short x86_64-unknown-linux-musl
#
# stormd depends on stormpull over ssh:// (a private repo), so the binaries
# are built here, on the build box, where cargo already has access — not in a
# container build stage — and test/Containerfile packages them FROM scratch,
# the same way the root Containerfiles package the release binaries.
#
# Needs the musl target and its linker (.cargo/config.toml), as for any
# stormd release build, and podman. With STAGE_ONLY=1 it stops after staging
# the context (test/.stage/) and prints its path, for a runner that builds
# images itself.
set -eu
suite=${1:-short}
target=${2:-x86_64-unknown-linux-musl}
root=$(cd "$(dirname "$0")/.." && pwd)
commit=$(git -C "$root" rev-parse HEAD)

cargo build --release --locked --target "$target" -p stormd -p stormd-test --manifest-path "$root/Cargo.toml"
tdir=$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" |
    sed 's/.*"target_directory":"\([^"]*\)".*/\1/')
stage="$root/test/.stage"
rm -rf "$stage"
mkdir -p "$stage"
cp "$tdir/$target/release/stormd" "$tdir/$target/release/stormd-test" "$stage/"
cp "$root/test/Containerfile" "$stage/"

if [ "${STAGE_ONLY:-0}" = 1 ]; then
    echo "$stage"
    exit 0
fi
podman build -f "$stage/Containerfile" --build-arg SUITE="$suite" --build-arg COMMIT="$commit" \
    -t "stormd-test-$suite" "$stage"
