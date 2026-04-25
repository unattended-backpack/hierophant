# Use the same base image as the main project for consistency. Petros ships
# the RISC Zero rust toolchain in the rzup-compatible layout that risc0-build
# discovers; see petros's Dockerfile for how $HOME/.risc0/ is set up.
ARG BUILD_IMAGE=unattended/petros:latest
FROM ${BUILD_IMAGE}

# Copy the integration test project. Cargo.lock is copied so the host binary
# build uses the pinned dep graph (mirrors the SP1 fibonacci Dockerfile). The
# guest directory IS copied and IS built; host/build.rs invokes risc0-build
# which cross-compiles guest/ to riscv32im-risc0-zkvm-elf using the RISC Zero
# rust toolchain from the petros image.
WORKDIR /test
COPY src/risc0-fibonacci/Cargo.toml Cargo.toml
COPY src/risc0-fibonacci/Cargo.lock Cargo.lock
COPY src/risc0-fibonacci/host/ host/
COPY src/risc0-fibonacci/guest/ guest/

# The shared `fibonacci` crate. The risc0-fibonacci guest + host path-dep to
# `../../fibonacci`, which from `/test/guest/` (or `/test/host/`) resolves to
# `/fibonacci/` inside this image. Cargo will error on workspace resolution
# if this directory does not exist (see the cross-VM shared-logic discussion
# in the repo's main README).
COPY src/fibonacci/ /fibonacci/

# Build the evm binary against the committed lockfile. The host's build.rs
# will shell out to cargo to cross-compile the guest; that nested invocation
# uses RISC0_BUILD_LOCKED=1 so it also respects the committed guest lockfile.
WORKDIR /test/host
ENV RISC0_BUILD_LOCKED=1
RUN cargo build --release --bin evm --locked

# Configure and run the test.
ENV RUST_LOG=info
ENV BONSAI_API_URL=http://hierophant:9010/bonsai
ENV BONSAI_API_KEY=
CMD ["cargo", "run", "--release", "--locked", "--bin", "evm", "--", "--n", "10"]
