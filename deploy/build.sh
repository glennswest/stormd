#!/bin/sh
# The whole build, as sc-build runs it: `sc-build deploy/build.sh`.
#
# Build and test the workspace (the stormd-test crate included), then clippy
# every target with warnings denied — the cross-project zero-warnings rule
# (stormcentral#193). Clippy was not in the build before stormd#77, which is
# how 37 findings piled up unseen.
set -eu
cargo build --locked --workspace
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
