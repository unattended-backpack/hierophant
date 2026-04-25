# Use the same base image as the main project for consistency.
ARG BUILD_IMAGE=unattended/petros:latest
FROM ${BUILD_IMAGE}

# Build-args needed to vendor the SP1 Groth16/plonk circuit artifacts. The
# evm binary calls `client.verify(&proof, &vk)` after receiving a wrapped
# proof; for the SNARK modes that path loads proving / verifying key assets
# via sp1-recursion-gnark-ffi, which reads them from $HOME/.sp1/circuits/.
# We vendor the same tarballs hierophant + contemplant use (from
# ${VENDOR_BASE_URL}/sp1/${SP1_CIRCUITS_VERSION}/{groth16,plonk}.tar.gz) and
# install them under /home/petros/.sp1/circuits/ so every test run verifies
# client-side without touching the network mid-test. The image is larger as
# a result, but only by the tarballs we'd already be shipping to the other
# two containers; the test-client context is entirely ephemeral anyway.
ARG VENDOR_BASE_URL
RUN test -n "${VENDOR_BASE_URL}" || ( \
  echo "ERROR: VENDOR_BASE_URL build argument is required!" >&2 \
  && exit 1)
ENV VENDOR_BASE_URL=${VENDOR_BASE_URL}

ARG SP1_CIRCUITS_VERSION
RUN test -n "${SP1_CIRCUITS_VERSION}" || ( \
  echo "ERROR: SP1_CIRCUITS_VERSION build argument is required!" >&2 \
  && exit 1)
ENV SP1_CIRCUITS_VERSION=${SP1_CIRCUITS_VERSION}

# Vendor the Groth16 and Plonk circuit artifacts FIRST, before the cargo
# build. The evm binary build doesn't consume the circuits; putting the
# vendor step above the source COPY + compile keeps ~2.5 GB of circuit
# bytes cached across routine test-client source edits. Install to
# /home/petros/.sp1/circuits/<mode>/<ver>/ where sp1-recursion-gnark-ffi
# expects them and place the .download_complete marker sp1-sdk checks
# before it falls back to the (busybox-tar-incompatible) S3 download
# path. We don't clean up /tmp/vendor.sh or /tmp/provers afterwards ;
# COPY lands those as root-owned, this stage runs as the non-root petros
# user, so `rm` would fail; and unlike Dockerfile.contemplant's multi-
# stage layout, this image is single-stage so /tmp residue would stay
# either way. The residue is a few KB of sha256 files plus a shell
# script; the test-client image is ephemeral.
WORKDIR /tmp
COPY container/vendor.sh /tmp/vendor.sh
COPY provers/ /tmp/provers/
RUN /tmp/vendor.sh "groth16.tar.gz" "provers/sp1/${SP1_CIRCUITS_VERSION}" "sp1/${SP1_CIRCUITS_VERSION}/" && \
  /tmp/vendor.sh "plonk.tar.gz" "provers/sp1/${SP1_CIRCUITS_VERSION}" "sp1/${SP1_CIRCUITS_VERSION}/" && \
  mkdir -p /home/petros/.sp1/circuits/groth16/${SP1_CIRCUITS_VERSION} && \
  mkdir -p /home/petros/.sp1/circuits/plonk/${SP1_CIRCUITS_VERSION} && \
  cp -r /tmp/extracted-groth16/${SP1_CIRCUITS_VERSION}/* /home/petros/.sp1/circuits/groth16/${SP1_CIRCUITS_VERSION}/ && \
  cp -r /tmp/extracted-plonk/${SP1_CIRCUITS_VERSION}/* /home/petros/.sp1/circuits/plonk/${SP1_CIRCUITS_VERSION}/ && \
  touch /home/petros/.sp1/circuits/groth16/.download_complete && \
  touch /home/petros/.sp1/circuits/plonk/.download_complete

# Copy the integration test project.
# Cargo.lock is copied so `cargo prove build` and the evm binary build use the
# pinned dep graph instead of freshly resolving and pulling crate versions that
# require a newer rustc than the Succinct toolchain ships with (e.g.
# constant_time_eq@0.4.3, which requires rustc 1.95 while Succinct currently
# ships 1.88-dev).
WORKDIR /test
COPY src/sp1-fibonacci/Cargo.toml Cargo.toml
COPY src/sp1-fibonacci/Cargo.lock Cargo.lock
COPY src/sp1-fibonacci/lib/ lib/
COPY src/sp1-fibonacci/program/ program/
COPY src/sp1-fibonacci/script/ script/

# The shared `fibonacci` crate. The sp1-fibonacci program + script path-dep
# to `../../fibonacci`, which from `/test/program/` resolves to `/fibonacci/`
# inside this image. Cargo will error on workspace resolution if this
# directory does not exist (see the cross-VM shared-logic discussion in
# the repo's main README).
COPY src/fibonacci/ /fibonacci/

# Build the ZK program (this creates the ELF file).
WORKDIR /test/program
RUN cargo prove build --locked

# Build the evm binary.
WORKDIR /test/script
RUN cargo build --release --bin evm --locked

# Configure and run the test.
# There are no assets on that private key.
WORKDIR /test/script
ENV SP1_PROVER=network
ENV NETWORK_RPC_URL=http://hierophant:9000
ENV NETWORK_PRIVATE_KEY=9edaae0d468d0dbb808b2f4ac28b4319186ab9824b9f28d9ac1ad99f8868bb96
ENV RUST_LOG=info
CMD ["cargo", "run", "--release", "--bin", "evm", "--", "--n", "10", "--system", "plonk"]
