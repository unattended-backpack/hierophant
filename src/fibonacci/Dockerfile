# Use the same base image as the main project for consistency.
ARG BUILD_IMAGE=unattended/petros:latest
FROM ${BUILD_IMAGE}

# Copy the integration test project.
WORKDIR /test
COPY Cargo.toml Cargo.toml
COPY lib/ lib/
COPY program/ program/
COPY script/ script/

# Build the ZK program (this creates the ELF file).
WORKDIR /test/program
RUN cargo prove build

# Build the evm binary.
WORKDIR /test/script
RUN cargo build --release --bin evm

# Configure and run the test.
# There are no assets on that private key.
ENV SP1_PROVER=network
ENV NETWORK_RPC_URL=http://hierophant:9000
ENV NETWORK_PRIVATE_KEY=9edaae0d468d0dbb808b2f4ac28b4319186ab9824b9f28d9ac1ad99f8868bb96
ENV RUST_LOG=info
CMD ["cargo", "run", "--release", "--bin", "evm", "--", "--n", "10", "--system", "plonk"]
