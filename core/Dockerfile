# ── Build stage ──────────────────────────────────────────────────────────────
# Toolchain is PINNED (rust:1.98.0-slim) — must stay in sync with
# scripts/cargo-check.sh and core/rust-toolchain.toml (channel 1.98.0).
FROM rust:1.98.0-slim AS builder

# CN-friendly mirrors: static.rust-lang.org / crates.io are slow or unreachable from
# some networks — route rustup and cargo through rsproxy.cn.
ENV RUSTUP_DIST_SERVER=https://rsproxy.cn \
    RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
RUN mkdir -p "$CARGO_HOME" && \
    printf '[source.crates-io]\nreplace-with = "rsproxy"\n[source.rsproxy]\nregistry = "sparse+https://rsproxy.cn/index/"\n[net]\ngit-fetch-with-cli = true\n' > "$CARGO_HOME/config.toml"

WORKDIR /build

# Copy the whole workspace and build the core binary in release mode.
COPY . .
# BuildKit cache mounts keep the cargo registry and compiled deps across builds, so a
# one-line source change doesn't re-download the world or recompile every dependency.
# The binary is copied out of the cached target dir into /out (cache-mount contents do
# not become part of the image), then the runtime stage copies it from there.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin otw-core \
    && mkdir -p /out \
    && cp /build/target/release/otw-core /out/otw-core

# Pre-create the upload dir owned by the runtime user (65532). When the named
# volume is first mounted here it inherits this ownership, so the non-root
# process can write to it. Distroless has no shell to chown at runtime.
RUN mkdir -p /data/uploads && chown -R 65532:65532 /data

# ── Runtime stage ────────────────────────────────────────────────────────────
# Distroless: tiny, no shell, smaller attack surface (M11 security-by-design).
# gcr.io is often unreachable from CN/blocked networks — use the DaoCloud mirror.
FROM gcr.m.daocloud.io/distroless/cc-debian12 AS runtime

WORKDIR /app
COPY --from=builder /out/otw-core /app/otw-core
# Carry over the upload dir with its 65532 ownership for the volume mountpoint.
COPY --from=builder --chown=65532:65532 /data /data

# Defaults; overridden by the compose environment / .env.
ENV OTW_CORE_HOST=0.0.0.0 \
    OTW_CORE_PORT=8080 \
    OTW_LOG=info

EXPOSE 8080
USER 65532:65532

ENTRYPOINT ["/app/otw-core"]
