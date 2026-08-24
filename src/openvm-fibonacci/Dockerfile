# Use the same base image as the main project for consistency. Petros ships
# the OpenVM guest toolchain (the stock nightly openvm-build pins, plus
# rust-src) pre-materialized under $RUSTUP_HOME with its install manifests,
# so openvm-build's `rustup component list` preflight and its
# `cargo +nightly-...` invocations both resolve offline; see petros's
# Dockerfile for how that layout is set up. (Earlier revisions of this image
# built on the official rust image and let openvm-build download the nightly
# through rustup at build time - exactly the unpinned-toolchain exception the
# petros arrangement exists to eliminate.)
ARG BUILD_IMAGE=unattended/petros:latest
FROM ${BUILD_IMAGE}

# The vendored bare mirrors of the OpenVM git dependencies, fetched from the
# CDN and swapped in for the github URLs recorded in both committed
# lockfiles. Mirror of the Dockerfile.contemplant arrangement (see the
# comment there for why git-fetch-with-cli lives in the cargo config FILE);
# the host build below AND the nested guest build both fetch openvm git
# deps, and both route through these mirrors.
ARG VENDOR_BASE_URL
RUN test -n "${VENDOR_BASE_URL}" || ( \
  echo "ERROR: VENDOR_BASE_URL build argument is required!" >&2 \
  && exit 1)
ENV VENDOR_BASE_URL=${VENDOR_BASE_URL}
ARG OPENVM_GIT_VERSION
RUN test -n "${OPENVM_GIT_VERSION}" || ( \
  echo "ERROR: OPENVM_GIT_VERSION build argument is required!" >&2 \
  && exit 1)
COPY container/vendor.sh /tmp/vendor.sh
COPY provers/openvm/git/${OPENVM_GIT_VERSION}/ /tmp/provers/openvm/git/${OPENVM_GIT_VERSION}/
RUN /tmp/vendor.sh "openvm-git-mirror.tar.gz" \
    "/tmp/provers/openvm/git/${OPENVM_GIT_VERSION}" \
    "openvm/git/${OPENVM_GIT_VERSION}/" && \
  /tmp/vendor.sh "stark-backend-git-mirror.tar.gz" \
    "/tmp/provers/openvm/git/${OPENVM_GIT_VERSION}" \
    "openvm/git/${OPENVM_GIT_VERSION}/" && \
  mkdir -p "$HOME/vendor-git" && \
  mv /tmp/extracted-openvm-git-mirror/openvm.git "$HOME/vendor-git/openvm.git" && \
  mv /tmp/extracted-stark-backend-git-mirror/stark-backend.git "$HOME/vendor-git/stark-backend.git" && \
  git config --global \
    url."file://$HOME/vendor-git/openvm.git".insteadOf \
    "https://github.com/openvm-org/openvm.git" && \
  git config --global \
    url."file://$HOME/vendor-git/stark-backend.git".insteadOf \
    "https://github.com/openvm-org/stark-backend.git" && \
  mkdir -p "${CARGO_HOME:-$HOME/.cargo}" && \
  printf '[net]\ngit-fetch-with-cli = true\n' \
    >> "${CARGO_HOME:-$HOME/.cargo}/config.toml"

# Copy the integration test project. Cargo.lock is copied so the host binary
# build uses the pinned dep graph (mirrors the risc0 fibonacci Dockerfile).
# The guest directory IS copied and IS built; host/build.rs invokes
# openvm-build which cross-compiles guest/ to riscv32im-risc0-zkvm-elf
# (OpenVM reuses the risc0 rustc target) using the petros-vendored OpenVM
# nightly toolchain.
WORKDIR /test
# --chown: COPY writes root-owned files by default, but the build runs as
# petros's unprivileged user (uid 10001), and openvm-build creates the guest
# target dir inside guest/ - root-owned sources make that a PermissionDenied
# panic in the host build.rs.
COPY --chown=10001:10001 src/openvm-fibonacci/Cargo.toml Cargo.toml
COPY --chown=10001:10001 src/openvm-fibonacci/Cargo.lock Cargo.lock
COPY --chown=10001:10001 src/openvm-fibonacci/host/ host/
COPY --chown=10001:10001 src/openvm-fibonacci/guest/ guest/

# The shared `fibonacci` crate. The openvm-fibonacci guest + host path-dep to
# `../../fibonacci`, which from `/test/guest/` (or `/test/host/`) resolves to
# `/fibonacci/` inside this image. Cargo will error on workspace resolution
# if this directory does not exist (see the cross-VM shared-logic discussion
# in the repo's main README).
COPY --chown=10001:10001 src/fibonacci/ /fibonacci/

# Build the host binary against the committed lockfile. The host's build.rs
# will shell out to cargo to cross-compile the guest; that nested invocation
# uses OPENVM_BUILD_LOCKED so it also respects the committed guest lockfile.
WORKDIR /test/host
ENV OPENVM_BUILD_LOCKED=1
RUN cargo build --release --bin host --locked

# Configure and run the test.
ENV RUST_LOG=info
CMD ["cargo", "run", "--release", "--locked", "--bin", "host", "--", "--n", "10"]
