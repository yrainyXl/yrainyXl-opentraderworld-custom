#!/usr/bin/env bash
#
# Standardized Rust development check for the OpenTraderWorld custom branch.
#
# Reuses the persistent Docker volumes `otw-check-cargo` (CARGO_HOME /
# registry cache) and `otw-check-target` (CARGO_TARGET_DIR) so every agent
# session gets a warm cache instead of a cold compile. No random volumes,
# no per-session cold starts.
#
# Toolchain consistency (do not drift):
#   - image:      rust:1.98.0-slim  (this script)
#   - builder:    core/Dockerfile   (same image tag)
#   - project:    core/rust-toolchain.toml (channel 1.98.0)
#
# Network: crates + rustup components route through rsproxy.cn (verified
# reachable from the server network).
#
# Usage:
#   scripts/cargo-check.sh                       # cargo check -p otw-core
#   scripts/cargo-check.sh -p otw-core --features x   # extra cargo args
#
# Exit code is the container's cargo exit code (no piping, no tail).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_IMAGE="${RUST_IMAGE:-rust:1.98.0-slim}"

# Cargo arguments — default to the main package.
if [ "$#" -eq 0 ]; then
    set -- -p otw-core
fi

exec docker run --rm \
    -e RUSTUP_DIST_SERVER=https://rsproxy.cn \
    -e RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup \
    -e CARGO_TARGET_DIR=/target \
    -v "${REPO_ROOT}:/build:ro" \
    -v "otw-check-cargo:/usr/local/cargo" \
    -v "otw-check-target:/target" \
    -w /build/core \
    "${RUST_IMAGE}" \
    bash -c '
        set -euo pipefail
        # Fixed rsproxy source config (idempotent).
        if [ ! -f "$CARGO_HOME/config.toml" ]; then
            printf "%s\n" \
                "[source.crates-io]" \
                "replace-with = \"rsproxy\"" \
                "[source.rsproxy]" \
                "registry = \"sparse+https://rsproxy.cn/index/\"" \
                "[net]" \
                "git-fetch-with-cli = true" \
                > "$CARGO_HOME/config.toml"
        fi
        exec cargo check "$@"
    ' _ "$@"
