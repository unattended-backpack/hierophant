# Unlike the sp1/risc0 test clients, this image does NOT build inside petros:
# openvm-build compiles the guest by shelling out to `cargo +<pinned-nightly>`
# and auto-installs that toolchain (plus rust-src) through rustup, which
# petros does not ship. The official rust image provides rustup out of the
# box; 1.91 matches openvm v2.0.0's host MSRV (rust-version = "1.91.1").
ARG BUILD_IMAGE=rust:1.91-bookworm
FROM ${BUILD_IMAGE}

# Copy the integration test project. Cargo.lock is copied so the host binary
# build uses the pinned dep graph (mirrors the risc0 fibonacci Dockerfile).
# The guest directory IS copied and IS built; host/build.rs invokes
# openvm-build which cross-compiles guest/ to riscv32im-risc0-zkvm-elf
# (OpenVM reuses the risc0 rustc target) using the pinned OpenVM nightly
# toolchain, installed via rustup on first build.
WORKDIR /test
COPY src/openvm-fibonacci/Cargo.toml Cargo.toml
COPY src/openvm-fibonacci/Cargo.lock Cargo.lock
COPY src/openvm-fibonacci/host/ host/
COPY src/openvm-fibonacci/guest/ guest/

# The shared `fibonacci` crate. The openvm-fibonacci guest + host path-dep to
# `../../fibonacci`, which from `/test/guest/` (or `/test/host/`) resolves to
# `/fibonacci/` inside this image. Cargo will error on workspace resolution
# if this directory does not exist (see the cross-VM shared-logic discussion
# in the repo's main README).
COPY src/fibonacci/ /fibonacci/

# Pre-install the pinned OpenVM guest toolchain so the (network-dependent)
# rustup step lands in its own cacheable layer instead of inside the cargo
# build. openvm-build would auto-install it otherwise; keep the version in
# lockstep with the openvm tag in host/Cargo.toml (v2.0.0 pins
# nightly-2026-01-18, overridable via OPENVM_RUST_TOOLCHAIN).
RUN rustup toolchain install nightly-2026-01-18 --component rust-src

# Build the host binary against the committed lockfile. The host's build.rs
# will shell out to cargo to cross-compile the guest; that nested invocation
# uses OPENVM_BUILD_LOCKED so it also respects the committed guest lockfile.
WORKDIR /test/host
ENV OPENVM_BUILD_LOCKED=1
RUN cargo build --release --bin host --locked

# Configure and run the test.
ENV RUST_LOG=info
CMD ["cargo", "run", "--release", "--locked", "--bin", "host", "--", "--n", "10"]
